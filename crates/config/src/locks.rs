//! Who is editing an object file right now (DESIGN §4.1): marker files
//! next to the file, from Watcher itself (web UI and `watcher config edit`)
//! and from editors that leave one (vim, nano, emacs, Kate).
//!
//! The locks are advisory. An editor that leaves no marker (VS Code, sed,
//! editing a copy and uploading it) is not seen; the hash check on every
//! write is what keeps changes from being lost.

use crate::files::{self, Error};
use std::io::{Read, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Marker file of a Watcher lock: `.hosts.toml.watcher-lock`.
pub const SUFFIX: &str = ".watcher-lock";

/// What holds a lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// The web UI (or an API client holding a lease).
    Web,
    /// `watcher config edit`.
    Cli,
    Vim,
    Nano,
    Emacs,
    Kate,
}

impl Via {
    pub fn as_str(self) -> &'static str {
        match self {
            Via::Web => "web",
            Via::Cli => "cli",
            Via::Vim => "vim",
            Via::Nano => "nano",
            Via::Emacs => "emacs",
            Via::Kate => "kate",
        }
    }

    fn parse(s: &str) -> Option<Via> {
        Some(match s {
            "web" => Via::Web,
            "cli" => Via::Cli,
            _ => return None,
        })
    }

    /// For messages: "in the web UI", "in vim".
    pub fn describe(self) -> &'static str {
        match self {
            Via::Web => "in the web UI",
            Via::Cli => "with watcher config edit",
            Via::Vim => "in vim",
            Via::Nano => "in nano",
            Via::Emacs => "in emacs",
            Via::Kate => "in Kate",
        }
    }
}

/// Someone editing a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Holder {
    /// The object file, relative to the object directory.
    pub path: String,
    /// User name; empty if the marker doesn't say.
    pub by: String,
    pub via: Via,
    pub since: Option<SystemTime>,
    pub pid: Option<u32>,
    pub host: Option<String>,
}

impl Holder {
    /// `jan is editing hosts.toml in vim`.
    pub fn sentence(&self) -> String {
        let who = if self.by.is_empty() {
            "someone"
        } else {
            &self.by
        };
        format!("{who} is editing {} {}", self.path, self.via.describe())
    }
}

/// The marker file Watcher writes for `file`.
pub fn lock_path(file: &Path) -> PathBuf {
    let name = file.file_name().unwrap_or_default().to_string_lossy();
    file.with_file_name(format!(".{name}{SUFFIX}"))
}

/// Live holders of `rel`, from the markers next to it.
pub fn find(dir: &Path, rel: &str) -> Result<Vec<Holder>, Error> {
    let file = files::resolve(dir, rel)?;
    let parent = file.parent().unwrap_or(dir);
    Ok(markers_in(dir, parent)
        .into_iter()
        .filter(|h| h.path == rel)
        .collect())
}

/// Live holders of every file below `dir`.
pub fn scan(dir: &Path) -> Vec<Holder> {
    let mut out = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(d) = pending.pop() {
        out.extend(markers_in(dir, &d));
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            if !hidden && entry.file_type().is_ok_and(|t| t.is_dir()) {
                pending.push(entry.path());
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Writes Watcher's marker for `rel`. With `exclusive`, fails if one is
/// there already (`AlreadyExists`).
pub fn write(dir: &Path, rel: &str, holder: &Holder, exclusive: bool) -> std::io::Result<()> {
    let file = files::resolve(dir, rel).map_err(std::io::Error::other)?;
    let mut table = toml::Table::new();
    table.insert("by".into(), holder.by.clone().into());
    table.insert("via".into(), holder.via.as_str().into());
    if let Some(since) = holder.since {
        let secs = since.duration_since(UNIX_EPOCH).unwrap_or_default();
        table.insert("since".into(), (secs.as_secs() as i64).into());
    }
    if let Some(pid) = holder.pid {
        table.insert("pid".into(), i64::from(pid).into());
    }
    if let Some(host) = &holder.host {
        table.insert("host".into(), host.clone().into());
    }
    let text = format!(
        "# {} (Watcher edit lock; removed when done)\n{}",
        holder.sentence(),
        toml::to_string(&table).map_err(std::io::Error::other)?
    );
    let mut options = std::fs::OpenOptions::new();
    options.write(true);
    if exclusive {
        options.create_new(true);
    } else {
        options.create(true).truncate(true);
    }
    options.open(lock_path(&file))?.write_all(text.as_bytes())
}

/// Removes Watcher's marker for `rel` if process `pid` wrote it as `via`.
pub fn remove(dir: &Path, rel: &str, via: Via, pid: u32) {
    let Ok(file) = files::resolve(dir, rel) else {
        return;
    };
    let marker = lock_path(&file);
    if read_watcher(&marker, rel).is_some_and(|h| h.via == via && h.pid == Some(pid)) {
        let _ = std::fs::remove_file(marker);
    }
}

/// Removes Watcher markers below `dir` whose process is gone, e.g. after
/// a crash. Returns the files they were for.
pub fn remove_stale(dir: &Path) -> Vec<String> {
    let mut removed = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(d) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with('.') {
                if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    pending.push(entry.path());
                }
                continue;
            }
            let Some(toml_name) = name.strip_prefix('.').and_then(|n| n.strip_suffix(SUFFIX))
            else {
                continue;
            };
            let Some(rel) = files::relative(dir, &d.join(toml_name)) else {
                continue;
            };
            if read_watcher(&entry.path(), &rel).is_some_and(|h| !alive(h.pid, h.host.as_deref()))
                && std::fs::remove_file(entry.path()).is_ok()
            {
                removed.push(rel);
            }
        }
    }
    removed
}

/// This machine's host name, if known.
pub fn local_host() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
}

fn short_host(h: &str) -> String {
    h.split('.').next().unwrap_or(h).to_ascii_lowercase()
}

/// Whether process `pid` on `host` still runs. Processes on other hosts
/// (a shared file system) and systems without `/proc` count as running.
pub fn alive(pid: Option<u32>, host: Option<&str>) -> bool {
    let Some(pid) = pid else {
        return true;
    };
    if pid == 0 {
        return false;
    }
    if let (Some(theirs), Some(ours)) = (host, local_host())
        && !theirs.is_empty()
        && short_host(theirs) != short_host(&ours)
    {
        return true;
    }
    let proc = Path::new("/proc");
    !proc.is_dir() || proc.join(pid.to_string()).exists()
}

/// Live markers in directory `d` (below the object directory `dir`).
fn markers_in(dir: &Path, d: &Path) -> Vec<Holder> {
    let Ok(entries) = std::fs::read_dir(d) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some((toml_name, kind)) = marker_name(&name) else {
            continue;
        };
        let Some(rel) = files::relative(dir, &d.join(toml_name)) else {
            continue;
        };
        let path = entry.path();
        let holder = match kind {
            Marker::Watcher => read_watcher(&path, &rel),
            Marker::Swap => read_swap(&path, &rel),
            Marker::Emacs => read_emacs(&path, &rel),
            Marker::Kate => Some(Holder {
                path: rel,
                by: String::new(),
                via: Via::Kate,
                since: created(&path),
                pid: None,
                host: None,
            }),
        };
        if let Some(h) = holder
            && alive(h.pid, h.host.as_deref())
        {
            out.push(h);
        }
    }
    out
}

enum Marker {
    Watcher,
    /// vim or nano: the same header layout.
    Swap,
    Emacs,
    Kate,
}

/// The object file a marker is for, and its kind.
fn marker_name(name: &str) -> Option<(&str, Marker)> {
    if let Some(file) = name.strip_prefix(".#") {
        return file.ends_with(".toml").then_some((file, Marker::Emacs));
    }
    let rest = name.strip_prefix('.')?;
    let (file, kind) = if let Some(f) = rest.strip_suffix(SUFFIX) {
        (f, Marker::Watcher)
    } else if let Some(f) = rest.strip_suffix(".kate-swp") {
        (f, Marker::Kate)
    } else {
        let f = [".swp", ".swo", ".swn"]
            .iter()
            .find_map(|s| rest.strip_suffix(s))?;
        (f, Marker::Swap)
    };
    file.ends_with(".toml").then_some((file, kind))
}

fn created(path: &Path) -> Option<SystemTime> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    meta.created().or_else(|_| meta.modified()).ok()
}

fn read_watcher(path: &Path, rel: &str) -> Option<Holder> {
    let text = std::fs::read_to_string(path).ok()?;
    let table: toml::Table = text.parse().ok()?;
    let str_of = |k: &str| table.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let int_of = |k: &str| table.get(k).and_then(toml::Value::as_integer);
    Some(Holder {
        path: rel.to_string(),
        by: str_of("by").unwrap_or_default(),
        via: Via::parse(&str_of("via")?)?,
        since: int_of("since")
            .and_then(|s| u64::try_from(s).ok())
            .map(|s| UNIX_EPOCH + Duration::from_secs(s)),
        pid: int_of("pid").and_then(|p| u32::try_from(p).ok()),
        host: str_of("host"),
    })
}

/// A vim swap file or nano lock file: `b0`, the program and version, the
/// process id (little endian) at 24, user at 28 and host at 68.
fn read_swap(path: &Path, rel: &str) -> Option<Holder> {
    let mut head = [0u8; 108];
    std::fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
    if &head[..2] != b"b0" {
        return None;
    }
    let via = if head[2..].starts_with(b"VIM") {
        Via::Vim
    } else if head[2..].starts_with(b"nano") {
        Via::Nano
    } else {
        return None;
    };
    let text = |range: std::ops::Range<usize>| {
        let bytes = &head[range];
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        String::from_utf8_lossy(&bytes[..end]).into_owned()
    };
    let host = text(68..108);
    Some(Holder {
        path: rel.to_string(),
        by: text(28..68),
        via,
        since: created(path),
        pid: Some(u32::from_le_bytes(head[24..28].try_into().ok()?)),
        host: (!host.is_empty()).then_some(host),
    })
}

/// An emacs lock: a symlink (or file) `.#name` → `user@host.pid:boot`.
fn read_emacs(path: &Path, rel: &str) -> Option<Holder> {
    let target = match std::fs::read_link(path) {
        Ok(t) => t.to_string_lossy().into_owned(),
        Err(_) => std::fs::read_to_string(path).ok()?,
    };
    let (by, rest) = target.split_once('@')?;
    let rest = rest.split(':').next().unwrap_or(rest);
    let (host, pid) = rest.rsplit_once('.')?;
    Some(Holder {
        path: rel.to_string(),
        by: by.to_string(),
        via: Via::Emacs,
        since: created(path),
        pid: pid.parse().ok(),
        host: Some(host.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("watcher-locks-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/hosts.toml"), "").unwrap();
        dir
    }

    fn me() -> u32 {
        std::process::id()
    }

    #[test]
    fn watcher_marker_round_trip() {
        let dir = tmp("watcher");
        let holder = Holder {
            path: "sub/hosts.toml".into(),
            by: "jan".into(),
            via: Via::Cli,
            since: Some(UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
            pid: Some(me()),
            host: local_host(),
        };
        write(&dir, "sub/hosts.toml", &holder, true).unwrap();
        assert!(dir.join("sub/.hosts.toml.watcher-lock").exists());
        // A second exclusive writer loses.
        let err = write(&dir, "sub/hosts.toml", &holder, true).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(find(&dir, "sub/hosts.toml").unwrap(), vec![holder.clone()]);
        assert_eq!(scan(&dir), vec![holder]);
        // Only the writer removes it.
        remove(&dir, "sub/hosts.toml", Via::Cli, me() + 1);
        remove(&dir, "sub/hosts.toml", Via::Web, me());
        assert_eq!(scan(&dir).len(), 1);
        remove(&dir, "sub/hosts.toml", Via::Cli, me());
        assert!(scan(&dir).is_empty());
    }

    fn swap(program: &[u8], pid: u32, user: &str, host: &str) -> Vec<u8> {
        let mut head = vec![0u8; 1024];
        head[..2].copy_from_slice(b"b0");
        head[2..2 + program.len()].copy_from_slice(program);
        head[24..28].copy_from_slice(&pid.to_le_bytes());
        head[28..28 + user.len()].copy_from_slice(user.as_bytes());
        head[68..68 + host.len()].copy_from_slice(host.as_bytes());
        head
    }

    #[test]
    fn vim_and_nano_swap_files() {
        let dir = tmp("swap");
        let host = local_host().unwrap_or_default();
        std::fs::write(
            dir.join("sub/.hosts.toml.swp"),
            swap(b"VIM 9.1", me(), "jan", &host),
        )
        .unwrap();
        let found = find(&dir, "sub/hosts.toml").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].via, found[0].by.as_str()), (Via::Vim, "jan"));
        assert_eq!(found[0].pid, Some(me()));

        std::fs::write(
            dir.join("sub/.hosts.toml.swp"),
            swap(b"nano 8.2", me(), "root", &host),
        )
        .unwrap();
        assert_eq!(find(&dir, "sub/hosts.toml").unwrap()[0].via, Via::Nano);

        // Left behind by a crash: the process is gone.
        std::fs::write(
            dir.join("sub/.hosts.toml.swp"),
            swap(b"VIM 9.1", 0, "jan", &host),
        )
        .unwrap();
        assert!(find(&dir, "sub/hosts.toml").unwrap().is_empty());

        // Not a swap file.
        std::fs::write(dir.join("sub/.hosts.toml.swp"), b"something else entirely").unwrap();
        assert!(find(&dir, "sub/hosts.toml").unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn emacs_lock() {
        let dir = tmp("emacs");
        let host = local_host().unwrap_or_else(|| "box".into());
        std::os::unix::fs::symlink(
            format!("jan@{host}.{}:1700000000", me()),
            dir.join("sub/.#hosts.toml"),
        )
        .unwrap();
        let found = scan(&dir);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].via, found[0].by.as_str()), (Via::Emacs, "jan"));
        assert_eq!(found[0].host.as_deref(), Some(host.as_str()));
    }

    #[test]
    fn stale_watcher_markers_are_removed() {
        let dir = tmp("stale");
        let mut holder = Holder {
            path: "sub/hosts.toml".into(),
            by: "jan".into(),
            via: Via::Web,
            since: None,
            pid: Some(0),
            host: local_host(),
        };
        write(&dir, "sub/hosts.toml", &holder, false).unwrap();
        assert!(scan(&dir).is_empty());
        assert_eq!(remove_stale(&dir), vec!["sub/hosts.toml".to_string()]);
        assert!(!dir.join("sub/.hosts.toml.watcher-lock").exists());

        holder.pid = Some(me());
        write(&dir, "sub/hosts.toml", &holder, false).unwrap();
        assert!(remove_stale(&dir).is_empty());
    }

    #[test]
    fn markers_of_other_files_are_ignored() {
        assert!(marker_name(".notes.txt.swp").is_none());
        assert!(marker_name(".#notes.txt").is_none());
        assert!(marker_name("hosts.toml").is_none());
        assert!(marker_name(".hosts.toml.swp").is_some());
    }
}
