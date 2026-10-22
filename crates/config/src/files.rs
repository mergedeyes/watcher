//! Reading and writing object files for the web UI and API: paths relative
//! to the object directory, content hashes for conflict detection, and
//! validated, atomic writes.

use crate::{Diagnostic, Loaded, load::discover, load::load_dir_with};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};

/// An object file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct File {
    /// Relative to the object directory, `/`-separated.
    pub path: String,
    pub hash: String,
    pub text: String,
}

/// A change to one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    /// Relative to the object directory, `/`-separated.
    pub path: String,
    /// Hash of the file the change was made on; `None` creates a new file.
    pub base: Option<String>,
    /// The new text; `None` deletes the file.
    pub text: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid path {0:?}: expected a relative path to a .toml file")]
    Path(String),
    #[error("{path} was changed by someone else")]
    Conflict {
        path: String,
        /// The file as it is now; `None` if it no longer exists.
        current: Option<File>,
    },
    #[error("the configuration has errors")]
    Invalid(Vec<Diagnostic>),
    #[error("cannot write {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
}

/// SHA-256 of `text`, hex.
pub fn hash(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// `path` relative to `dir`, `/`-separated; `None` if it is outside.
pub fn relative(dir: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(dir).ok()?;
    let parts: Option<Vec<&str>> = rel
        .components()
        .map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();
    Some(parts?.join("/"))
}

/// The file `rel` below `dir`. Rejects anything that could leave `dir`, or
/// that the loader would not read (hidden entries, other extensions).
pub fn resolve(dir: &Path, rel: &str) -> Result<PathBuf, Error> {
    let invalid = || Error::Path(rel.to_string());
    if !rel.ends_with(".toml") || rel.contains('\\') {
        return Err(invalid());
    }
    let mut path = dir.to_path_buf();
    for part in rel.split('/') {
        if part.is_empty() || part.starts_with('.') || part.contains('\0') {
            return Err(invalid());
        }
        path.push(part);
    }
    Ok(path)
}

/// All object files, sorted by path.
pub fn list(dir: &Path) -> std::io::Result<Vec<File>> {
    discover(dir)?
        .into_iter()
        .filter_map(|p| Some((relative(dir, &p)?, p)))
        .map(|(path, full)| {
            let text = std::fs::read_to_string(&full)?;
            Ok(File {
                hash: hash(&text),
                path,
                text,
            })
        })
        .collect()
}

/// The file `rel`, or `None` if it doesn't exist.
pub fn read(dir: &Path, rel: &str) -> Result<Option<File>, Error> {
    let path = resolve(dir, rel)?;
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(Some(File {
            path: rel.to_string(),
            hash: hash(&text),
            text,
        })),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(Error::Io {
            path: rel.to_string(),
            source,
        }),
    }
}

/// Checks that every change was made on the current file.
pub fn check_base(dir: &Path, changes: &[Change]) -> Result<(), Error> {
    for change in changes {
        let current = read(dir, &change.path)?;
        if current.as_ref().map(|f| &f.hash) != change.base.as_ref() {
            return Err(Error::Conflict {
                path: change.path.clone(),
                current,
            });
        }
    }
    Ok(())
}

/// Compiles the configuration as it would be after `changes`, without
/// writing anything.
pub fn preview(dir: &Path, changes: &[Change]) -> Result<Loaded, Error> {
    let mut overlay = BTreeMap::new();
    for change in changes {
        overlay.insert(resolve(dir, &change.path)?, change.text.clone());
    }
    load_dir_with(dir, &overlay).map_err(Error::Invalid)
}

/// Applies `changes` if they were made on the current files and the result
/// is valid. Each file is replaced atomically; new directories are created.
pub fn apply(dir: &Path, changes: &[Change]) -> Result<Loaded, Error> {
    check_base(dir, changes)?;
    let loaded = preview(dir, changes)?;
    for change in changes {
        let path = resolve(dir, &change.path)?;
        let io = |source| Error::Io {
            path: change.path.clone(),
            source,
        };
        match &change.text {
            Some(text) => write_atomic(&path, text).map_err(io)?,
            None => match std::fs::remove_file(&path) {
                Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(io(err)),
                _ => {}
            },
        }
    }
    Ok(loaded)
}

/// Writes through a temporary file in the same directory, so readers see
/// the old or the new file, never a partial one. Keeps the permissions of
/// an existing file.
fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    // Hidden, so a concurrent load skips it.
    let tmp = dir.join(format!(".{name}.tmp"));
    let mut file = std::fs::File::create(&tmp)?;
    if let Ok(meta) = std::fs::metadata(path) {
        file.set_permissions(meta.permissions())?;
    }
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)?;
    // Make the rename durable too.
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("watcher-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const HOST: &str = "[[host]]\nname = \"web01\"\n";

    #[test]
    fn resolve_rejects_escapes() {
        let dir = Path::new("/etc/watcher/conf.d");
        assert_eq!(
            resolve(dir, "ui/hosts.toml").unwrap(),
            dir.join("ui/hosts.toml")
        );
        for bad in [
            "../x.toml",
            "/etc/passwd.toml",
            "a/../../x.toml",
            ".hidden.toml",
            "a//b.toml",
            "hosts.txt",
            "a\\b.toml",
            "",
        ] {
            assert!(resolve(dir, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn relative_paths_use_slashes() {
        let dir = Path::new("/c");
        assert_eq!(
            relative(dir, Path::new("/c/ui/hosts.toml")).as_deref(),
            Some("ui/hosts.toml")
        );
        assert_eq!(relative(dir, Path::new("/d/x.toml")), None);
    }

    #[test]
    fn apply_writes_valid_changes() {
        let dir = tempdir("apply");
        let change = Change {
            path: "ui/hosts.toml".into(),
            base: None,
            text: Some(HOST.into()),
        };
        let loaded = apply(&dir, &[change]).unwrap();
        assert_eq!(loaded.config.hosts[0].name, "web01");
        let file = read(&dir, "ui/hosts.toml").unwrap().unwrap();
        assert_eq!(file.text, HOST);
        assert_eq!(list(&dir).unwrap(), vec![file.clone()]);

        // Deleting needs the current hash.
        let delete = Change {
            path: "ui/hosts.toml".into(),
            base: Some(file.hash),
            text: None,
        };
        apply(&dir, &[delete]).unwrap();
        assert!(read(&dir, "ui/hosts.toml").unwrap().is_none());
    }

    #[test]
    fn apply_refuses_stale_and_invalid_changes() {
        let dir = tempdir("refuse");
        std::fs::write(dir.join("hosts.toml"), HOST).unwrap();

        let stale = Change {
            path: "hosts.toml".into(),
            base: Some(hash("old")),
            text: Some(String::new()),
        };
        match apply(&dir, &[stale]) {
            Err(Error::Conflict { path, current }) => {
                assert_eq!(path, "hosts.toml");
                assert_eq!(current.unwrap().text, HOST);
            }
            other => panic!("{other:?}"),
        }
        // Creating a file that exists is a conflict too.
        let exists = Change {
            path: "hosts.toml".into(),
            base: None,
            text: Some(String::new()),
        };
        assert!(matches!(
            apply(&dir, &[exists]),
            Err(Error::Conflict { .. })
        ));

        let duplicate = Change {
            path: "more.toml".into(),
            base: None,
            text: Some(HOST.into()),
        };
        match apply(&dir, &[duplicate]) {
            Err(Error::Invalid(diagnostics)) => {
                assert!(diagnostics[0].message.contains("already defined"))
            }
            other => panic!("{other:?}"),
        }
        assert!(!dir.join("more.toml").exists());
    }

    #[test]
    fn preview_sees_deletions() {
        let dir = tempdir("preview");
        std::fs::write(dir.join("hosts.toml"), HOST).unwrap();
        let delete = Change {
            path: "hosts.toml".into(),
            base: None,
            text: None,
        };
        let loaded = preview(&dir, &[delete]).unwrap();
        assert!(loaded.config.hosts.is_empty());
        assert!(dir.join("hosts.toml").exists());
    }
}
