//! Check directories: where bare command names (`check_ping`, no `/`) are
//! looked up before the server's PATH.
//!
//! Checks run with `PATH` set to the check directories followed by the
//! server's own PATH, so lookup works the same for direct commands and for
//! `shell = true`, and a script added to a directory works without a reload.

use crate::{Config, Diagnostic, Origin, RunOn, Severity};
use serde::Serialize;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use watcher_exec::CommandLine;

/// Plugin directories of common distributions and source installs.
pub const PLUGIN_DIRS: &[&str] = &[
    "/usr/lib/monitoring-plugins",
    "/usr/lib/nagios/plugins",
    "/usr/lib64/nagios/plugins",
    "/usr/local/libexec/monitoring-plugins",
    "/usr/local/nagios/libexec",
];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckDirs {
    dirs: Vec<PathBuf>,
}

/// An executable in a check directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Script {
    pub name: String,
    pub dir: PathBuf,
}

impl CheckDirs {
    pub fn new(dirs: Vec<PathBuf>) -> Self {
        CheckDirs { dirs }
    }

    /// `checks/` in `base` (the directory of watcher.toml), then the
    /// [`PLUGIN_DIRS`] that exist.
    pub fn default_for(base: &Path) -> Self {
        let mut dirs = vec![base.join("checks")];
        dirs.extend(PLUGIN_DIRS.iter().map(PathBuf::from).filter(|d| d.is_dir()));
        CheckDirs { dirs }
    }

    pub fn dirs(&self) -> &[PathBuf] {
        &self.dirs
    }

    /// `PATH` for check processes: the check directories, then the
    /// server's PATH.
    pub fn search_path(&self) -> OsString {
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        let paths = self
            .dirs
            .iter()
            .cloned()
            .chain(std::env::split_paths(&inherited));
        std::env::join_paths(paths).unwrap_or(inherited)
    }

    /// The file `program` runs, as found with [`search_path`](Self::search_path).
    /// Names with a `/` are taken as paths.
    pub fn find(&self, program: &str) -> Option<PathBuf> {
        if program.contains('/') {
            let path = PathBuf::from(program);
            return is_executable(&path).then_some(path);
        }
        std::env::split_paths(&self.search_path())
            .map(|dir| dir.join(program))
            .find(|path| is_executable(path))
    }

    /// Executables in the check directories, by name. A name in an earlier
    /// directory hides the same name in later ones.
    pub fn scripts(&self) -> Vec<Script> {
        let mut found = BTreeMap::new();
        for dir in &self.dirs {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(name) = entry.file_name().into_string() else {
                    continue;
                };
                if name.starts_with('.') || !is_executable(&entry.path()) {
                    continue;
                }
                found.entry(name).or_insert_with(|| dir.clone());
            }
        }
        found
            .into_iter()
            .map(|(name, dir)| Script { name, dir })
            .collect()
    }

    /// Warnings for services whose program doesn't exist, at the place the
    /// command is written (usually the check). Only direct commands are
    /// checked, not `shell = true` scripts, nor commands that run on the
    /// host without `copy`.
    pub fn missing_commands(&self, config: &Config) -> Vec<Diagnostic> {
        let mut found: BTreeMap<(Origin, String), String> = BTreeMap::new();
        for service in &config.services {
            let Some(CommandLine::Argv(argv)) = &service.command else {
                continue;
            };
            if service.settings.run_on == RunOn::Host && !service.settings.copy {
                continue;
            }
            let Some(program) = argv.first() else {
                continue;
            };
            let origin = service
                .sources
                .get("command")
                .map_or(&service.origin, |s| &s.origin);
            let what = match &service.check {
                Some(check) => format!("check {check:?}"),
                None => format!("service {:?}", service.id),
            };
            found
                .entry((origin.clone(), program.clone()))
                .or_insert(what);
        }
        found
            .into_iter()
            .filter(|((_, program), _)| self.find(program).is_none())
            .map(|((origin, program), what)| Diagnostic {
                origin: Some(origin),
                severity: Severity::Warning,
                message: format!("{what}: command not found: {program}"),
            })
            .collect()
    }
}

fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load_dir;

    fn tmp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("watcher-checkdirs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn script(dir: &Path, name: &str, mode: u32) {
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\necho OK\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn finds_scripts_in_order() {
        let tmp = tmp("order");
        let (a, b) = (tmp.join("a"), tmp.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        script(&a, "check_x", 0o755);
        script(&b, "check_x", 0o755);
        script(&b, "check_y", 0o755);
        script(&b, "utils.sh", 0o644);
        let dirs = CheckDirs::new(vec![a.clone(), b.clone(), tmp.join("missing")]);

        assert_eq!(dirs.find("check_x"), Some(a.join("check_x")));
        assert_eq!(dirs.find("check_y"), Some(b.join("check_y")));
        assert_eq!(dirs.find("utils.sh"), None);
        // PATH still applies after the check directories.
        assert!(dirs.find("sh").is_some());
        let names: Vec<_> = dirs
            .scripts()
            .into_iter()
            .map(|s| (s.name, s.dir))
            .collect();
        assert_eq!(names, [("check_x".into(), a), ("check_y".into(), b)]);
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[test]
    fn warns_about_missing_commands() {
        let tmp = tmp("missing");
        let conf = tmp.join("conf.d");
        std::fs::create_dir_all(&conf).unwrap();
        std::fs::create_dir_all(tmp.join("checks")).unwrap();
        script(&tmp.join("checks"), "check_here", 0o755);
        std::fs::write(
            conf.join("a.toml"),
            r#"
[check.here]
command = "check_here -H $host.address$"

[check.gone]
command = "check_gone"

[check.script]
shell = true
command = "check_gone | head -1"

[service.here]
[service.gone]
[service.script]

[hostgroup.all]
members = ["*"]
services = ["here", "gone", "script"]

[[host]]
name = "h1"

[[host]]
name = "h2"
"#,
        )
        .unwrap();
        let config = load_dir(&conf).unwrap().config;
        let dirs = CheckDirs::default_for(&tmp);
        let warnings = dirs.missing_commands(&config);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(
            warnings[0].message,
            r#"check "gone": command not found: check_gone"#
        );
        std::fs::remove_dir_all(tmp).unwrap();
    }
}
