//! What a new configuration changes compared to the running one.

use crate::{Config, Service};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Changes {
    pub hosts: Delta,
    pub services: Delta,
    pub contacts: Delta,
    pub sinks: Delta,
}

/// Names added, removed and changed, sorted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Delta {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
}

impl Delta {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
            && self.services.is_empty()
            && self.contacts.is_empty()
            && self.sinks.is_empty()
    }
}

/// Compares objects by what they do; where they are defined doesn't matter.
pub fn changes(old: &Config, new: &Config) -> Changes {
    Changes {
        hosts: delta(
            &old.hosts,
            &new.hosts,
            |h| h.name.clone(),
            |a, b| {
                let mut b = b.clone();
                b.origin = a.origin.clone();
                *a == b
            },
        ),
        services: delta(
            &old.services,
            &new.services,
            |s| s.id.clone(),
            |a, b| same_service(old, a, new, b),
        ),
        contacts: delta(
            &old.contacts,
            &new.contacts,
            |c| c.name.clone(),
            |a, b| a.email == b.email && a.webhook == b.webhook,
        ),
        sinks: delta(
            &old.sinks,
            &new.sinks,
            |s| s.name.clone(),
            |a, b| a.kind == b.kind && a.flush == b.flush && a.max_buffer == b.max_buffer,
        ),
    }
}

fn same_service(old: &Config, a: &Service, new: &Config, b: &Service) -> bool {
    let (x, y) = (old.check(a), new.check(b));
    a.command == b.command
        && a.check_hash == b.check_hash
        && x.interval == y.interval
        && x.retry_interval == y.retry_interval
        && x.max_attempts == y.max_attempts
        && x.timeout == y.timeout
        && x.freshness == y.freshness
        && x.thresholds == y.thresholds
        && x.notify == y.notify
}

fn delta<T>(
    old: &[T],
    new: &[T],
    key: impl Fn(&T) -> String,
    same: impl Fn(&T, &T) -> bool,
) -> Delta {
    let old: BTreeMap<String, &T> = old.iter().map(|t| (key(t), t)).collect();
    let new: BTreeMap<String, &T> = new.iter().map(|t| (key(t), t)).collect();
    let mut delta = Delta::default();
    for (name, n) in &new {
        match old.get(name) {
            None => delta.added.push(name.clone()),
            Some(o) if !same(o, n) => delta.changed.push(name.clone()),
            Some(_) => {}
        }
    }
    delta.removed = old
        .keys()
        .filter(|k| !new.contains_key(*k))
        .cloned()
        .collect();
    delta
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load_dir;

    fn config(text: &str) -> Config {
        let dir = std::env::temp_dir().join(format!(
            "watcher-diff-{}-{}",
            std::process::id(),
            crate::files::hash(text)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("c.toml"), text).unwrap();
        let config = load_dir(&dir).unwrap().config;
        std::fs::remove_dir_all(&dir).unwrap();
        config
    }

    #[test]
    fn reports_added_removed_and_changed() {
        let old = config(
            r#"
[[host]]
name = "a"
[[host]]
name = "b"
[[check]]
name = "ping"
apply_to = "true"
command = "check_ping -H $host.address$"
"#,
        );
        let new = config(
            r#"
[[host]]
name = "a"
address = "10.0.0.1"
[[host]]
name = "c"
[[check]]
name = "ping"
apply_to = "true"
command = "check_ping -H $host.address$"
"#,
        );
        let c = changes(&old, &new);
        assert_eq!(c.hosts.added, ["c"]);
        assert_eq!(c.hosts.removed, ["b"]);
        assert_eq!(c.hosts.changed, ["a"]);
        assert_eq!(c.services.added, ["c/ping"]);
        assert_eq!(c.services.removed, ["b/ping"]);
        // The address is in the command.
        assert_eq!(c.services.changed, ["a/ping"]);
        assert!(changes(&new, &new).is_empty());
    }
}
