//! The Config Tree: where an object and its values come from, and where it
//! is used. Every node that is an object names it (`kind` and `name`) so a
//! client can link to it, and carries the file and line it is written at.

use crate::{Assigned, Config, Host, Layer, NotifyRule, Origin, Reason, Service, Source, files};
use serde::Serialize;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Node {
    /// `host`, `service`, `role`, `check`, `hostgroup`, `servicegroup`,
    /// `contact`, `contactgroup` and `notify` for objects; `section` and
    /// `value` otherwise.
    pub kind: &'static str,
    /// The object's name (`host/service` for services), to link to it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub label: String,
    /// A value, or why something applies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Relative to the object directory when one was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "is_zero")]
    pub line: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Node>,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// Builds Config Trees, with file names relative to `base`.
pub struct Builder<'a> {
    config: &'a Config,
    base: Option<&'a Path>,
}

impl<'a> Builder<'a> {
    pub fn new(config: &'a Config, base: Option<&'a Path>) -> Self {
        Builder { config, base }
    }

    /// The tree of an object: `kind` as in [`Node::kind`], `name` as there.
    pub fn tree(&self, kind: &str, name: &str) -> Option<Node> {
        match kind {
            "host" => self.host(name),
            "service" => self.service(name),
            "role" => self.role(name),
            "check" => self.check(name),
            "hostgroup" => self.hostgroup(name),
            "servicegroup" => self.servicegroup(name),
            "contact" => self.contact(name),
            "contactgroup" => self.contactgroup(name),
            "notify" => self.rule(name),
            _ => None,
        }
    }

    fn node(
        &self,
        kind: &'static str,
        name: Option<&str>,
        label: impl Into<String>,
        origin: Option<&Origin>,
    ) -> Node {
        let (file, line) = match origin {
            Some(o) => (Some(self.file(o)), o.line),
            None => (None, 0),
        };
        Node {
            kind,
            name: name.map(String::from),
            label: label.into(),
            detail: None,
            file,
            line,
            children: Vec::new(),
        }
    }

    fn file(&self, origin: &Origin) -> String {
        self.base
            .and_then(|b| files::relative(b, &origin.file))
            .unwrap_or_else(|| origin.file.display().to_string())
    }

    fn section(&self, label: &str, children: Vec<Node>) -> Option<Node> {
        (!children.is_empty()).then(|| {
            let mut n = self.node("section", None, label, None);
            n.children = children;
            n
        })
    }

    /// A node for the object a value came from.
    fn source(&self, source: &Source) -> Node {
        let (kind, name, label): (&'static str, Option<String>, String) = match source.layer {
            Layer::Default => ("value", None, "default".into()),
            Layer::Check => (
                "check",
                Some(source.name.clone()),
                format!("check {}", source.name),
            ),
            Layer::Role => (
                "role",
                Some(source.name.clone()),
                format!("role {}", source.name),
            ),
            Layer::RoleService => {
                let role = source
                    .name
                    .split('.')
                    .next()
                    .unwrap_or_default()
                    .to_string();
                let label = format!(
                    "role {role}, service {}",
                    source.name.split_once('.').map_or("", |x| x.1)
                );
                ("role", Some(role), label)
            }
            Layer::Host | Layer::ForEach | Layer::HostService => {
                let host = source
                    .name
                    .split('.')
                    .next()
                    .unwrap_or_default()
                    .to_string();
                let label = match source.layer {
                    Layer::ForEach => format!(
                        "host {host}, {}",
                        source.name.split_once('.').map_or("", |x| x.1)
                    ),
                    Layer::HostService => format!(
                        "host {host}, service {}",
                        source.name.split_once('.').map_or("", |x| x.1)
                    ),
                    _ => format!("host {host}"),
                };
                ("host", Some(host), label)
            }
        };
        self.node(kind, name.as_deref(), label, Some(&source.origin))
    }

    fn reason(&self, reason: &Reason) -> (String, Option<Origin>) {
        match reason {
            Reason::Listed { by, origin } => (format!("listed on {by}"), Some(origin.clone())),
            Reason::Selector { origin, matched } => {
                (format!("matched: {matched}"), Some(origin.clone()))
            }
            Reason::Included { by } => (format!("included by role {by}"), None),
        }
    }

    fn assigned(&self, kind: &'static str, a: &Assigned) -> Node {
        let (why, origin) = self.reason(&a.reason);
        let mut n = self.node(
            kind,
            Some(&a.name),
            format!("{kind} {}", a.name),
            origin.as_ref(),
        );
        n.detail = Some(why);
        n
    }

    /// One effective value and where it came from.
    fn value(&self, key: &str, value: String, source: Option<&Source>) -> Node {
        let mut n = self.node("value", None, key, source.map(|s| &s.origin));
        n.detail = Some(value);
        n.children.push(match source {
            Some(s) => self.source(s),
            None => self.node("value", None, "default", None),
        });
        n
    }

    fn service(&self, id: &str) -> Option<Node> {
        let config = self.config;
        let s = config.service(id)?;
        let host = config.host(s);
        let mut root = self.node("service", Some(&s.id), &s.id, Some(&s.origin));

        let mut from = Vec::new();
        let mut h = self.node(
            "host",
            Some(&host.name),
            format!("host {}", host.name),
            Some(&host.origin),
        );
        if let Some((var, key)) = &s.for_each {
            let mut item = self.node(
                "value",
                None,
                format!("vars.{var}.{key}"),
                Some(&host.origin),
            );
            item.detail = Some("for_each entry".into());
            h.children.push(item);
        }
        from.push(h);
        if let Some(role) = &s.role
            && let Some(a) = host.roles.iter().find(|r| r.name == *role)
        {
            let mut r = self.assigned("role", a);
            for layer in s.layers.iter().filter(|l| l.layer == Layer::RoleService) {
                r.children.push(self.source(layer));
            }
            from.push(r);
        }
        if let Some(check) = s
            .check
            .as_ref()
            .and_then(|c| config.checks.iter().find(|x| x.name == *c))
        {
            let mut c = self.node(
                "check",
                Some(&check.name),
                format!("check {}", check.name),
                Some(&check.origin),
            );
            if let Some(cmd) = &check.command {
                let mut v = self.node("value", None, "command", None);
                v.detail = Some(cmd.clone());
                c.children.push(v);
            }
            from.push(c);
        }
        root.children.extend(self.section("from", from));
        root.children
            .extend(self.section("values", self.service_values(s)));
        root.children.extend(
            self.section(
                "groups",
                s.groups
                    .iter()
                    .map(|g| self.assigned("servicegroup", g))
                    .collect(),
            ),
        );
        root.children.extend(self.section(
            "notifications",
            self.rules(config.matching_rules(s, config.is_host_check(s))),
        ));
        Some(root)
    }

    fn service_values(&self, s: &Service) -> Vec<Node> {
        let st = &s.settings;
        let src = |k: &str| s.sources.get(k);
        let mut out = Vec::new();
        if let Some(cmd) = &s.command {
            out.push(self.value("command", cmd.to_string(), src("command")));
        } else {
            out.push(self.value("passive", "true".into(), src("passive")));
        }
        out.push(self.value("interval", duration(st.interval), src("interval")));
        out.push(self.value(
            "retry_interval",
            duration(st.retry_interval),
            src("retry_interval"),
        ));
        out.push(self.value(
            "max_attempts",
            st.max_attempts.to_string(),
            src("max_attempts"),
        ));
        out.push(self.value("timeout", duration(st.timeout), src("timeout")));
        if let Some(f) = st.freshness {
            out.push(self.value("freshness", duration(f), src("freshness")));
        }
        out.push(self.value("format", st.format.to_string(), src("format")));
        let flap = match st.flap {
            Some(f) => format!("{}% / {}%", f.low, f.high),
            None => "off".into(),
        };
        out.push(self.value("flap_detection", flap, src("flap_detection")));
        for (metric, spec) in &st.thresholds {
            for (level, t) in [("warn", &spec.warn), ("crit", &spec.crit)] {
                if let Some(t) = t {
                    let key = format!("thresholds.{metric}.{level}");
                    out.push(self.value(&key, t.to_string(), src(&key)));
                }
            }
        }
        for (k, v) in &s.vars {
            let key = format!("vars.{k}");
            out.push(self.value(&key, v.clone(), src(&key)));
        }
        out
    }

    fn rules(&self, matched: Vec<(&NotifyRule, String)>) -> Vec<Node> {
        matched
            .into_iter()
            .map(|(rule, why)| {
                let label = rule.label();
                let mut n = self.node(
                    "notify",
                    Some(&label),
                    format!("notify {label}"),
                    Some(&rule.origin),
                );
                n.detail = Some(format!("matched: {why}"));
                n.children = self.rule_targets(rule);
                n
            })
            .collect()
    }

    fn rule_targets(&self, rule: &NotifyRule) -> Vec<Node> {
        let mut out = Vec::new();
        for c in &rule.contacts {
            let origin = self
                .config
                .contacts
                .iter()
                .find(|x| x.name == *c)
                .map(|x| &x.origin);
            out.push(self.node("contact", Some(c), format!("contact {c}"), origin));
        }
        for g in &rule.contact_groups {
            let origin = self
                .config
                .contactgroups
                .iter()
                .find(|x| x.name == *g)
                .map(|x| &x.origin);
            out.push(self.node("contactgroup", Some(g), format!("contactgroup {g}"), origin));
        }
        out
    }

    fn host(&self, name: &str) -> Option<Node> {
        let config = self.config;
        let (idx, host) = config
            .hosts
            .iter()
            .enumerate()
            .find(|(_, h)| h.name == name)?;
        let mut root = self.node("host", Some(&host.name), &host.name, Some(&host.origin));
        root.children.extend(
            self.section(
                "roles",
                host.roles
                    .iter()
                    .map(|r| self.assigned("role", r))
                    .collect(),
            ),
        );
        root.children
            .extend(self.section("values", self.host_values(host)));
        let services: Vec<Node> = config
            .services
            .iter()
            .filter(|s| s.host == idx)
            .map(|s| {
                let mut n = self.node(
                    "service",
                    Some(&s.id),
                    format!("service {}", s.name),
                    Some(&s.origin),
                );
                n.detail = Some(match (&s.role, &s.for_each) {
                    (Some(r), Some((var, key))) => format!("role {r}, for_each {var}.{key}"),
                    (Some(r), None) => format!("role {r}"),
                    (None, _) => "host".into(),
                });
                n
            })
            .collect();
        root.children.extend(self.section("services", services));
        root.children.extend(
            self.section(
                "groups",
                host.groups
                    .iter()
                    .map(|g| self.assigned("hostgroup", g))
                    .collect(),
            ),
        );
        let rules = match config.host_check(host) {
            Some(hc) => config.matching_rules(hc, true),
            None => config
                .notify
                .iter()
                .filter(|r| r.enabled && r.services.is_none())
                .filter_map(|r| {
                    let why = match &r.hosts {
                        Some(sel) => format!("host: {}", sel.matches(host)?),
                        None => "all".into(),
                    };
                    Some((r, why))
                })
                .collect(),
        };
        root.children
            .extend(self.section("notifications", self.rules(rules)));
        Some(root)
    }

    fn host_values(&self, host: &Host) -> Vec<Node> {
        let src = |k: &str| host.sources.get(k);
        let mut out = vec![self.value("address", host.address.clone(), src("address"))];
        if let Some(n) = &host.node {
            out.push(self.value("node", n.clone(), src("node")));
        }
        if let Some(hc) = &host.host_check {
            out.push(self.value("host_check", hc.clone(), src("host_check")));
        }
        if let Some(g) = host.graph_check() {
            out.push(self.value("graph", g.to_string(), src("graph").or(src("host_check"))));
        }
        if !host.tags.is_empty() {
            let mut n = self.value("tags", host.tags.join(", "), None);
            n.children = vec![self.node(
                "host",
                Some(&host.name),
                format!("host {}", host.name),
                Some(&host.origin),
            )];
            out.push(n);
        }
        for (k, v) in &host.vars {
            let key = format!("vars.{k}");
            let text = match v.as_text() {
                Some(t) => t.to_string(),
                None => "(table)".into(),
            };
            out.push(self.value(&key, text, src(&key)));
        }
        out
    }

    fn role(&self, name: &str) -> Option<Node> {
        let config = self.config;
        let role = config.roles.iter().find(|r| r.name == name)?;
        let mut root = self.node("role", Some(&role.name), &role.name, Some(&role.origin));
        let link = |n: &str| {
            let origin = config.roles.iter().find(|r| r.name == n).map(|r| &r.origin);
            self.node("role", Some(n), format!("role {n}"), origin)
        };
        root.children
            .extend(self.section("includes", role.include.iter().map(|r| link(r)).collect()));
        root.children.extend(
            self.section(
                "included by",
                config
                    .roles
                    .iter()
                    .filter(|r| r.include.contains(&role.name))
                    .map(|r| link(&r.name))
                    .collect(),
            ),
        );
        root.children.extend(
            self.section(
                "hosts",
                config
                    .hosts
                    .iter()
                    .filter_map(|h| {
                        let a = h.roles.iter().find(|r| r.name == role.name)?;
                        let (why, origin) = self.reason(&a.reason);
                        let mut n = self.node(
                            "host",
                            Some(&h.name),
                            format!("host {}", h.name),
                            origin.as_ref().or(Some(&h.origin)),
                        );
                        n.detail = Some(why);
                        Some(n)
                    })
                    .collect(),
            ),
        );
        root.children.extend(
            self.section(
                "services",
                role.services
                    .iter()
                    .map(|s| {
                        let check = config.services.iter().find(|x| {
                            x.role.as_deref() == Some(role.name.as_str())
                                && (x.name == *s
                                    || x.for_each.is_some() && x.name.starts_with(&format!("{s}_")))
                        });
                        let mut n = self.node(
                            "value",
                            None,
                            format!("service {s}"),
                            check.map(|c| &c.origin),
                        );
                        n.detail = check
                            .and_then(|c| c.check.clone())
                            .map(|c| format!("check {c}"));
                        n
                    })
                    .collect(),
            ),
        );
        Some(root)
    }

    fn check(&self, name: &str) -> Option<Node> {
        let config = self.config;
        let check = config.checks.iter().find(|c| c.name == name)?;
        let mut root = self.node("check", Some(&check.name), &check.name, Some(&check.origin));
        if let Some(cmd) = &check.command {
            let mut v = self.node("value", None, "command", Some(&check.origin));
            v.detail = Some(cmd.clone());
            root.children.push(v);
        }
        root.children.extend(
            self.section(
                "services",
                config
                    .services
                    .iter()
                    .filter(|s| s.check.as_deref() == Some(name))
                    .map(|s| {
                        let mut n = self.node(
                            "service",
                            Some(&s.id),
                            format!("service {}", s.id),
                            Some(&s.origin),
                        );
                        n.detail = s.role.as_ref().map(|r| format!("role {r}"));
                        n
                    })
                    .collect(),
            ),
        );
        Some(root)
    }

    fn hostgroup(&self, name: &str) -> Option<Node> {
        let g = self.config.hostgroups.iter().find(|g| g.name == name)?;
        let mut root = self.node(
            "hostgroup",
            Some(&g.name),
            g.title.as_deref().unwrap_or(&g.name),
            Some(&g.origin),
        );
        root.children.extend(
            self.section(
                "members",
                self.config
                    .hosts
                    .iter()
                    .filter_map(|h| {
                        let a = h.groups.iter().find(|x| x.name == name)?;
                        let (why, origin) = self.reason(&a.reason);
                        let mut n = self.node(
                            "host",
                            Some(&h.name),
                            format!("host {}", h.name),
                            origin.as_ref(),
                        );
                        n.detail = Some(why);
                        Some(n)
                    })
                    .collect(),
            ),
        );
        Some(root)
    }

    fn servicegroup(&self, name: &str) -> Option<Node> {
        let g = self.config.servicegroups.iter().find(|g| g.name == name)?;
        let mut root = self.node(
            "servicegroup",
            Some(&g.name),
            g.title.as_deref().unwrap_or(&g.name),
            Some(&g.origin),
        );
        root.children.extend(
            self.section(
                "members",
                self.config
                    .services
                    .iter()
                    .filter_map(|s| {
                        let a = s.groups.iter().find(|x| x.name == name)?;
                        let (why, origin) = self.reason(&a.reason);
                        let mut n = self.node(
                            "service",
                            Some(&s.id),
                            format!("service {}", s.id),
                            origin.as_ref(),
                        );
                        n.detail = Some(why);
                        Some(n)
                    })
                    .collect(),
            ),
        );
        Some(root)
    }

    fn contact(&self, name: &str) -> Option<Node> {
        let c = self.config.contacts.iter().find(|c| c.name == name)?;
        let mut root = self.node("contact", Some(&c.name), &c.name, Some(&c.origin));
        let mut values = Vec::new();
        for e in &c.email {
            let mut v = self.node("value", None, "email", None);
            v.detail = Some(e.clone());
            values.push(v);
        }
        if let Some(w) = &c.webhook {
            let mut v = self.node("value", None, "webhook", None);
            v.detail = Some(w.clone());
            values.push(v);
        }
        root.children.extend(self.section("values", values));
        root.children.extend(
            self.section(
                "groups",
                c.groups
                    .iter()
                    .map(|g| self.assigned("contactgroup", g))
                    .collect(),
            ),
        );
        let rules: Vec<&NotifyRule> = self
            .config
            .notify
            .iter()
            .filter(|r| {
                r.contacts.contains(&c.name)
                    || r.contact_groups
                        .iter()
                        .any(|g| c.groups.iter().any(|x| x.name == *g))
            })
            .collect();
        root.children.extend(self.section(
            "notify rules",
            rules.iter().map(|r| self.rule_link(r)).collect(),
        ));
        Some(root)
    }

    fn contactgroup(&self, name: &str) -> Option<Node> {
        let g = self.config.contactgroups.iter().find(|g| g.name == name)?;
        let mut root = self.node(
            "contactgroup",
            Some(&g.name),
            g.title.as_deref().unwrap_or(&g.name),
            Some(&g.origin),
        );
        root.children.extend(
            self.section(
                "members",
                self.config
                    .contacts
                    .iter()
                    .filter_map(|c| {
                        let a = c.groups.iter().find(|x| x.name == name)?;
                        let (why, origin) = self.reason(&a.reason);
                        let mut n = self.node(
                            "contact",
                            Some(&c.name),
                            format!("contact {}", c.name),
                            origin.as_ref(),
                        );
                        n.detail = Some(why);
                        Some(n)
                    })
                    .collect(),
            ),
        );
        let rules: Vec<Node> = self
            .config
            .notify
            .iter()
            .filter(|r| r.contact_groups.iter().any(|x| x == name))
            .map(|r| self.rule_link(r))
            .collect();
        root.children.extend(self.section("notify rules", rules));
        Some(root)
    }

    fn rule_link(&self, rule: &NotifyRule) -> Node {
        let label = rule.label();
        self.node(
            "notify",
            Some(&label),
            format!("notify {label}"),
            Some(&rule.origin),
        )
    }

    fn rule(&self, label: &str) -> Option<Node> {
        let config = self.config;
        let rule = config.notify.iter().find(|r| r.label() == label)?;
        let mut root = self.rule_link(rule);
        root.label = label.to_string();
        root.children
            .extend(self.section("to", self.rule_targets(rule)));
        let mut matched = Vec::new();
        for s in &config.services {
            let host_alert = config.is_host_check(s);
            if let Some((_, why)) = config
                .matching_rules(s, host_alert)
                .into_iter()
                .find(|(r, _)| r.label() == label)
            {
                let (kind, name, text) = match host_alert {
                    true => (
                        "host",
                        config.host(s).name.clone(),
                        format!("host {}", config.host(s).name),
                    ),
                    false => ("service", s.id.clone(), format!("service {}", s.id)),
                };
                let mut n = self.node(kind, Some(&name), text, None);
                n.detail = Some(why);
                matched.push(n);
            }
        }
        let total = matched.len();
        matched.truncate(200);
        let title = match total > 200 {
            true => format!("matches ({total}, first 200)"),
            false => "matches".to_string(),
        };
        root.children.extend(self.section(&title, matched));
        Some(root)
    }
}

fn duration(d: Duration) -> String {
    humantime::format_duration(d).to_string()
}

/// The tree as indented text, for the terminal.
pub fn render(node: &Node) -> String {
    fn walk(node: &Node, prefix: &str, last: bool, root: bool, out: &mut String) {
        let mut line = String::new();
        if !root {
            line.push_str(prefix);
            line.push_str(if last { "└─ " } else { "├─ " });
        }
        line.push_str(&node.label);
        if let Some(d) = &node.detail {
            line.push_str(": ");
            line.push_str(d);
        }
        if let Some(f) = &node.file {
            let place = match node.line {
                0 => f.clone(),
                n => format!("{f}:{n}"),
            };
            let width = 60usize.saturating_sub(line.chars().count()).max(2);
            line.push_str(&" ".repeat(width));
            line.push_str(&place);
        }
        out.push_str(&line);
        out.push('\n');
        let child_prefix = match root {
            true => String::new(),
            false => format!("{prefix}{}", if last { "   " } else { "│  " }),
        };
        for (i, c) in node.children.iter().enumerate() {
            walk(c, &child_prefix, i + 1 == node.children.len(), false, out);
        }
    }
    let mut out = String::new();
    walk(node, "", true, true, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load_dir;

    #[test]
    fn service_tree_shows_sources() {
        let dir = std::env::temp_dir().join(format!("watcher-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("a.toml"),
            r#"[check.disk]
command = "check_disk -w $warn$ -p $path$"
vars.warn = "20%"

[role.linux]
interval = "2m"
service.disk = { for_each = "disks" }

[contact.ops]
email = "ops@example.com"

[[notify]]
contacts = ["ops"]
hosts = { tags = ["prod"] }

[[host]]
name = "db01"
roles = ["linux"]
tags = ["prod"]
vars.disks.data = { path = "/srv", warn = "5%" }
"#,
        )
        .unwrap();
        let config = load_dir(&dir).unwrap().config;
        let tree = Builder::new(&config, Some(&dir))
            .tree("service", "db01/disk_data")
            .unwrap();
        let text = render(&tree);
        std::fs::remove_dir_all(&dir).unwrap();
        for expected in [
            "db01/disk_data",
            "role linux: listed on host db01",
            "role linux, service disk",
            "check disk",
            "interval: 2m",
            "vars.warn: 5%",
            "host db01, vars.disks.data",
            "notify rule at",
            "matched: host: tag prod",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in\n{text}");
        }
        assert!(text.contains("a.toml:7"), "{text}");
    }
}
