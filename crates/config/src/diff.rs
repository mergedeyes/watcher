//! What a new configuration changes compared to the running one.

use crate::{Assigned, Config, Host, NotifyRule, Service};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Changes {
    pub hosts: Delta,
    pub services: Delta,
    /// Host, service and contact groups, as `hostgroup x`, ...
    pub groups: Delta,
    pub contacts: Delta,
    /// Notify rules, by name or place.
    pub notify: Delta,
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
            && self.groups.is_empty()
            && self.contacts.is_empty()
            && self.notify.is_empty()
            && self.sinks.is_empty()
    }
}

/// Compares objects by what they do; where they are defined doesn't matter.
pub fn changes(old: &Config, new: &Config) -> Changes {
    let groups = |c: &Config| -> Vec<(String, Option<String>)> {
        let mut out = Vec::new();
        for (kind, list) in [
            ("hostgroup", &c.hostgroups),
            ("servicegroup", &c.servicegroups),
            ("contactgroup", &c.contactgroups),
        ] {
            out.extend(
                list.iter()
                    .map(|g| (format!("{kind} {}", g.name), g.title.clone())),
            );
        }
        out
    };
    Changes {
        hosts: delta(&old.hosts, &new.hosts, |h| h.name.clone(), same_host),
        services: delta(&old.services, &new.services, |s| s.id.clone(), same_service),
        groups: delta(&groups(old), &groups(new), |g| g.0.clone(), |a, b| a == b),
        contacts: delta(
            &old.contacts,
            &new.contacts,
            |c| c.name.clone(),
            |a, b| {
                a.email == b.email && a.webhook == b.webhook && names(&a.groups) == names(&b.groups)
            },
        ),
        notify: delta(&old.notify, &new.notify, NotifyRule::label, same_rule),
        sinks: delta(
            &old.sinks,
            &new.sinks,
            |s| s.name.clone(),
            |a, b| a.kind == b.kind && a.flush == b.flush && a.max_buffer == b.max_buffer,
        ),
    }
}

fn names(list: &[Assigned]) -> Vec<&str> {
    let mut out: Vec<&str> = list.iter().map(|a| a.name.as_str()).collect();
    out.sort();
    out
}

fn same_host(a: &Host, b: &Host) -> bool {
    a.address == b.address
        && a.node == b.node
        && names(&a.roles) == names(&b.roles)
        && a.tags == b.tags
        && names(&a.groups) == names(&b.groups)
        && a.vars == b.vars
        && a.host_check == b.host_check
        && a.graph == b.graph
        && a.enabled == b.enabled
}

fn same_service(a: &Service, b: &Service) -> bool {
    a.command == b.command
        && a.check_hash == b.check_hash
        && a.settings == b.settings
        && names(&a.groups) == names(&b.groups)
}

fn same_rule(a: &NotifyRule, b: &NotifyRule) -> bool {
    // Selectors have no equality; their source text is part of the file, so
    // compare what is cheap and treat a moved rule as unchanged.
    a.contacts == b.contacts
        && a.contact_groups == b.contact_groups
        && a.states == b.states
        && a.on == b.on
        && a.enabled == b.enabled
        && a.hosts.is_some() == b.hosts.is_some()
        && a.services.is_some() == b.services.is_some()
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
[check.ping]
command = "check_ping -H $host.address$"
[role.base]
hosts = {}
service.ping = {}
[[host]]
name = "a"
[[host]]
name = "b"
"#,
        );
        let new = config(
            r#"
[check.ping]
command = "check_ping -H $host.address$"
[role.base]
hosts = {}
service.ping = {}
[hostgroup.all]
members = {}
[[host]]
name = "a"
address = "10.0.0.1"
[[host]]
name = "c"
"#,
        );
        let c = changes(&old, &new);
        assert_eq!(c.hosts.added, ["c"]);
        assert_eq!(c.hosts.removed, ["b"]);
        // The address, and the new host group.
        assert_eq!(c.hosts.changed, ["a"]);
        assert_eq!(c.services.added, ["c/ping"]);
        assert_eq!(c.services.removed, ["b/ping"]);
        // The address is in the command.
        assert_eq!(c.services.changed, ["a/ping"]);
        assert_eq!(c.groups.added, ["hostgroup all"]);
        assert!(changes(&new, &new).is_empty());
    }
}
