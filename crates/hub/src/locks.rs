//! Edit leases (DESIGN §4.1): who has an object file open for editing in
//! the web UI. A lease is renewed while the page is open and lapses after
//! [`TTL`] without renewal. Each lease is mirrored to a marker file next to
//! the object file, so `watcher config edit` on the server sees it too.

use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};
use tracing::info;
use watcher_config::locks::{self, Holder, Via};

/// A lease lapses this long after its last renewal (the UI renews every
/// 30 seconds).
pub const TTL: Duration = Duration::from_secs(120);

struct Lease {
    id: String,
    by: String,
    since: SystemTime,
    seen: Instant,
}

pub struct Leases {
    dir: PathBuf,
    pid: u32,
    host: Option<String>,
    leases: Mutex<HashMap<String, Lease>>,
}

/// A lease taken or renewed.
#[derive(Debug)]
pub struct Granted {
    pub id: String,
    /// Editors that have the file open as well (after taking it over).
    pub others: Vec<Holder>,
}

impl Leases {
    /// Leases on the files below `dir`. Removes markers left by a Watcher
    /// that is no longer running.
    pub fn new(dir: PathBuf) -> Self {
        let stale = locks::remove_stale(&dir);
        if !stale.is_empty() {
            info!("removed stale edit locks: {}", stale.join(", "));
        }
        Leases {
            dir,
            pid: std::process::id(),
            host: locks::local_host(),
            leases: Mutex::new(HashMap::new()),
        }
    }

    fn holder(&self, path: &str, lease: &Lease) -> Holder {
        Holder {
            path: path.to_string(),
            by: lease.by.clone(),
            via: Via::Web,
            since: Some(lease.since),
            pid: Some(self.pid),
            host: self.host.clone(),
        }
    }

    /// Our own markers are leases, not outside editors.
    fn ours(&self, h: &Holder) -> bool {
        h.via == Via::Web && h.pid == Some(self.pid)
    }

    fn outside(&self, path: &str) -> Vec<Holder> {
        locks::find(&self.dir, path)
            .unwrap_or_default()
            .into_iter()
            .filter(|h| !self.ours(h))
            .collect()
    }

    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<String, Lease>> {
        let mut map = self.leases.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        map.retain(|path, lease| {
            let live = now.duration_since(lease.seen) < TTL;
            if !live {
                locks::remove(&self.dir, path, Via::Web, self.pid);
            }
            live
        });
        map
    }

    /// Drops lapsed leases and their markers.
    pub fn sweep(&self) {
        drop(self.map());
    }

    /// Takes or renews the lease on `path` for `by`. `id` renews a lease
    /// held already; `force` takes the file over from whoever has it.
    /// Err: who has it.
    pub fn acquire(
        &self,
        path: &str,
        by: &str,
        id: Option<&str>,
        force: bool,
    ) -> Result<Granted, Holder> {
        let mut map = self.map();
        if let Some(lease) = map.get_mut(path) {
            if id == Some(lease.id.as_str()) {
                lease.seen = Instant::now();
                return Ok(Granted {
                    id: lease.id.clone(),
                    others: self.outside(path),
                });
            }
            if !force {
                return Err(self.holder(path, lease));
            }
        }
        let others = self.outside(path);
        if !force && let Some(h) = others.first() {
            return Err(h.clone());
        }
        let lease = Lease {
            id: new_id(),
            by: by.to_string(),
            since: SystemTime::now(),
            seen: Instant::now(),
        };
        // Leave a marker of `watcher config edit` in place: it removes it.
        if !others.iter().any(|h| matches!(h.via, Via::Web | Via::Cli)) {
            let holder = self.holder(path, &lease);
            if let Err(err) = locks::write(&self.dir, path, &holder, false) {
                tracing::warn!("cannot write the edit lock of {path}: {err}");
            }
        }
        let granted = Granted {
            id: lease.id.clone(),
            others,
        };
        map.insert(path.to_string(), lease);
        Ok(granted)
    }

    /// Gives up the lease `id` on `path`.
    pub fn release(&self, path: &str, id: &str) {
        let mut map = self.map();
        if map.get(path).is_some_and(|l| l.id == id) {
            map.remove(path);
            locks::remove(&self.dir, path, Via::Web, self.pid);
        }
    }

    /// Whether a writer holding the leases `ids` may change `paths`: no one
    /// else has a lease on them, and files open in an editor need a lease
    /// (taken with force). Err: who has a file.
    pub fn check(&self, paths: &[String], ids: &[String]) -> Result<(), Holder> {
        let map = self.map();
        for path in paths {
            match map.get(path) {
                Some(lease) if ids.contains(&lease.id) => {}
                Some(lease) => return Err(self.holder(path, lease)),
                None => {
                    if let Some(h) = self.outside(path).into_iter().next() {
                        return Err(h);
                    }
                }
            }
        }
        Ok(())
    }

    /// Everyone editing a file: leases and outside editors.
    pub fn list(&self) -> Vec<Holder> {
        let map = self.map();
        let mut all: Vec<Holder> = map.iter().map(|(p, l)| self.holder(p, l)).collect();
        all.extend(locks::scan(&self.dir).into_iter().filter(|h| !self.ours(h)));
        all.sort_by(|a, b| a.path.cmp(&b.path));
        all
    }
}

fn new_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the OS random number generator works");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A holder for the API.
pub fn holder_json(h: &Holder) -> Value {
    json!({
        "path": h.path,
        "by": h.by,
        "via": h.via.as_str(),
        "since": h.since.and_then(|t| jiff::Timestamp::try_from(t).ok()).map(|t| t.to_string()),
        "pid": h.pid,
        "host": h.host,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leases(name: &str) -> (Leases, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("watcher-leases-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("hosts.toml"), "").unwrap();
        (Leases::new(dir.clone()), dir)
    }

    #[test]
    fn one_editor_at_a_time() {
        let (l, dir) = leases("one");
        let a = l.acquire("hosts.toml", "anna", None, false).unwrap();
        assert!(dir.join(".hosts.toml.watcher-lock").exists());
        // Renewing keeps the lease; someone else is refused.
        assert_eq!(
            l.acquire("hosts.toml", "anna", Some(&a.id), false)
                .unwrap()
                .id,
            a.id
        );
        let held = l.acquire("hosts.toml", "ben", None, false).unwrap_err();
        assert_eq!((held.by.as_str(), held.via), ("anna", Via::Web));
        // Writes need the lease.
        let paths = ["hosts.toml".to_string()];
        assert!(l.check(&paths, &[]).is_err());
        assert!(l.check(&paths, std::slice::from_ref(&a.id)).is_ok());
        // Taking over: anna's renewal now fails.
        let b = l.acquire("hosts.toml", "ben", None, true).unwrap();
        assert_eq!(
            l.acquire("hosts.toml", "anna", Some(&a.id), false)
                .unwrap_err()
                .by,
            "ben"
        );
        assert_eq!(l.list().len(), 1);
        l.release("hosts.toml", &a.id);
        assert_eq!(l.list().len(), 1);
        l.release("hosts.toml", &b.id);
        assert!(l.list().is_empty());
        assert!(!dir.join(".hosts.toml.watcher-lock").exists());
        assert!(l.check(&paths, &[]).is_ok());
    }

    #[test]
    fn outside_editors() {
        let (l, dir) = leases("outside");
        let cli = Holder {
            path: "hosts.toml".into(),
            by: "root".into(),
            via: Via::Cli,
            since: None,
            pid: Some(std::process::id()),
            host: locks::local_host(),
        };
        locks::write(&dir, "hosts.toml", &cli, true).unwrap();
        let paths = ["hosts.toml".to_string()];
        assert_eq!(l.check(&paths, &[]).unwrap_err().via, Via::Cli);
        assert_eq!(
            l.acquire("hosts.toml", "anna", None, false)
                .unwrap_err()
                .via,
            Via::Cli
        );
        // Taken over: the CLI marker stays, the lease allows writes.
        let a = l.acquire("hosts.toml", "anna", None, true).unwrap();
        assert_eq!(a.others.len(), 1);
        assert!(l.check(&paths, std::slice::from_ref(&a.id)).is_ok());
        assert_eq!(l.list().len(), 2);
        l.release("hosts.toml", &a.id);
        assert!(dir.join(".hosts.toml.watcher-lock").exists());
    }
}
