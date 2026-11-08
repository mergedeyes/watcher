//! The Config Tree: where an object and its values come from, and where it
//! is used. Every node that is an object names it (`kind` and `name`) so a
//! client can link to it, and carries the file and line it is written at.

use crate::{
    Assigned, Config, Host, Layer, NotifyRule, Origin, Reason, RunOn, Service, Source, files,
};
use serde::Serialize;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Node {
    /// `host`, `service`, `check`, `hostgroup`, `servicegroup`, `contact`,
    /// `contactgroup`, `downtimegroup` and `notify` for objects; `section` and `value`
    /// otherwise. A `service` named `host/service` is a service on a host,
    /// one without `/` a service object.
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
    /// A `service` named `host/service` is a service on a host, otherwise
    /// a service object.
    pub fn tree(&self, kind: &str, name: &str) -> Option<Node> {
        match kind {
            "host" => self.host(name),
            "service" if name.contains('/') => self.service(name),
            "service" => self.service_object(name),
            "check" => self.check(name),
            "hostgroup" => self.hostgroup(name),
            "servicegroup" => self.servicegroup(name),
            "contact" => self.contact(name),
            "contactgroup" => self.contactgroup(name),
            "downtimegroup" => self.downtimegroup(name),
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
    /// A node for the object a value came from.
    fn source(&self, source: &Source) -> Node {
        let first = |s: &str| s.split('.').next().unwrap_or_default().to_string();
        let rest = |s: &str| s.split_once('.').map_or("", |x| x.1).to_string();
        let (kind, name, label): (&'static str, Option<String>, String) = match source.layer {
            Layer::Default => ("value", None, "default".into()),
            Layer::Check => (
                "check",
                Some(source.name.clone()),
                format!("check {}", source.name),
            ),
            Layer::Service => (
                "service",
                Some(source.name.clone()),
                format!("service {}", source.name),
            ),
            Layer::HostGroup => (
                "hostgroup",
                Some(source.name.clone()),
                format!("hostgroup {}", source.name),
            ),
            Layer::Host => ("host", Some(source.name.clone()), {
                format!("host {}", source.name)
            }),
            Layer::ForEach => {
                let host = first(&source.name);
                let label = format!("host {host}, {}", rest(&source.name));
                ("host", Some(host), label)
            }
            Layer::HostService => {
                let host = first(&source.name);
                let label = format!("host {host}, service {}", rest(&source.name));
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
        }
    }

    /// A node for the object that lists a service: the first words of a
    /// [`Reason::Listed`] `by` (`servicegroup basics on host web01`).
    fn lister(&self, reason: &Reason) -> Node {
        let (why, origin) = self.reason(reason);
        let by = match reason {
            Reason::Listed { by, .. } => by.as_str(),
            Reason::Selector { .. } => "",
        };
        let mut words = by.split_whitespace();
        let kind: Option<&'static str> = match words.next() {
            Some("host") => Some("host"),
            Some("hostgroup") => Some("hostgroup"),
            Some("servicegroup") => Some("servicegroup"),
            Some("service") => Some("service"),
            _ => None,
        };
        let name = words.next().map(|n| n.trim_end_matches(','));
        let mut n = match (kind, name) {
            (Some(kind), Some(name)) => self.node(kind, Some(name), by, origin.as_ref()),
            _ => self.node("value", None, why.clone(), origin.as_ref()),
        };
        if kind.is_some() {
            n.detail = Some("listed".into());
        }
        n
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
        if let Some(object) = s
            .object
            .as_ref()
            .and_then(|o| config.service_objects.iter().find(|x| x.name == *o))
        {
            let mut o = self.node(
                "service",
                Some(&object.name),
                format!("service {}", object.name),
                Some(&object.origin),
            );
            o.children = s.assigned.iter().map(|a| self.lister(&a.reason)).collect();
            from.push(o);
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
        if st.run_on == RunOn::Host {
            out.push(self.value("run_on", "host".into(), src("run_on")));
        }
        if st.copy {
            out.push(self.value("copy", "true".into(), src("copy")));
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
                let mut n = self.rule_link(rule);
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

    /// How a host got a service, in short: `hostgroup linux`, `host`.
    fn how(s: &Service) -> String {
        let mut how: Vec<String> = s
            .assigned
            .iter()
            .map(|a| match &a.reason {
                Reason::Listed { by, .. } => by.clone(),
                Reason::Selector { matched, .. } => matched.clone(),
            })
            .collect();
        if how.is_empty() {
            how.push("host".into());
        }
        if let Some((var, key)) = &s.for_each {
            how.push(format!("for_each {var}.{key}"));
        }
        how.join("; ")
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
                "groups",
                host.groups
                    .iter()
                    .map(|g| self.assigned("hostgroup", g))
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
                n.detail = Some(Self::how(s));
                n
            })
            .collect();
        root.children.extend(self.section("services", services));
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
        if let Some(ssh) = &host.ssh {
            out.push(self.value("ssh.user", ssh.user.clone(), src("ssh.user")));
            out.push(self.value("ssh.port", ssh.port.to_string(), src("ssh.port")));
            out.push(self.value("ssh.secret", ssh.secret.clone(), src("ssh.secret")));
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

    /// Services on hosts, with how each host got it.
    fn instances<'s>(&self, list: impl Iterator<Item = &'s Service>) -> Vec<Node> {
        list.map(|s| {
            let mut n = self.node(
                "service",
                Some(&s.id),
                format!("service {}", s.id),
                Some(&s.origin),
            );
            n.detail = Some(Self::how(s));
            n
        })
        .collect()
    }

    fn service_object(&self, name: &str) -> Option<Node> {
        let config = self.config;
        let object = config.service_objects.iter().find(|s| s.name == name)?;
        let mut root = self.node(
            "service",
            Some(&object.name),
            &object.name,
            Some(&object.origin),
        );
        if let Some(check) = object
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
            root.children.push(c);
        }
        let groups: Vec<Node> = config
            .servicegroups
            .iter()
            .filter_map(|g| {
                let (why, origin) = match (
                    g.services.contains(&object.name),
                    object.groups.contains(&g.name),
                ) {
                    (true, _) => (format!("listed on servicegroup {}", g.name), &g.origin),
                    (false, true) => (format!("listed on service {}", object.name), &object.origin),
                    (false, false) => return None,
                };
                let mut n = self.node(
                    "servicegroup",
                    Some(&g.name),
                    format!("servicegroup {}", g.name),
                    Some(origin),
                );
                n.detail = Some(why);
                Some(n)
            })
            .collect();
        root.children.extend(self.section("groups", groups));
        let instances: Vec<&Service> = config
            .services
            .iter()
            .filter(|s| s.object.as_deref() == Some(name))
            .collect();
        // Who assigns it, each once.
        let mut listers: Vec<Node> = Vec::new();
        for s in &instances {
            for a in &s.assigned {
                let n = self.lister(&a.reason);
                if !listers.iter().any(|l| l.label == n.label) {
                    listers.push(n);
                }
            }
        }
        root.children.extend(self.section("assigned by", listers));
        root.children
            .extend(self.section("hosts", self.instances(instances.into_iter())));
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
                    .service_objects
                    .iter()
                    .filter(|s| s.check.as_deref() == Some(name))
                    .map(|s| {
                        self.node(
                            "service",
                            Some(&s.name),
                            format!("service {}", s.name),
                            Some(&s.origin),
                        )
                    })
                    .collect(),
            ),
        );
        root.children.extend(
            self.section(
                "hosts",
                self.instances(
                    config
                        .services
                        .iter()
                        .filter(|s| s.check.as_deref() == Some(name)),
                ),
            ),
        );
        Some(root)
    }

    /// Links to the services and service groups a group lists.
    fn listed(&self, kind: &'static str, names: &[String], origin: &Origin) -> Vec<Node> {
        names
            .iter()
            .map(|n| {
                let at = match kind {
                    "service" => self
                        .config
                        .service_objects
                        .iter()
                        .find(|s| s.name == *n)
                        .map(|s| &s.origin),
                    _ => self
                        .config
                        .servicegroups
                        .iter()
                        .find(|g| g.name == *n)
                        .map(|g| &g.origin),
                };
                let mut node = self.node(kind, Some(n), format!("{kind} {n}"), at.or(Some(origin)));
                node.detail = Some("listed".into());
                node
            })
            .collect()
    }

    fn hostgroup(&self, name: &str) -> Option<Node> {
        let g = self.config.hostgroups.iter().find(|g| g.name == name)?;
        let mut root = self.node(
            "hostgroup",
            Some(&g.name),
            g.title.as_deref().unwrap_or(&g.name),
            Some(&g.origin),
        );
        root.children
            .extend(self.section("services", self.listed("service", &g.services, &g.origin)));
        root.children.extend(self.section(
            "servicegroups",
            self.listed("servicegroup", &g.servicegroups, &g.origin),
        ));
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
        let config = self.config;
        let g = config.servicegroups.iter().find(|g| g.name == name)?;
        let mut root = self.node(
            "servicegroup",
            Some(&g.name),
            g.title.as_deref().unwrap_or(&g.name),
            Some(&g.origin),
        );
        let mut services = self.listed("service", &g.services, &g.origin);
        for object in config
            .service_objects
            .iter()
            .filter(|s| s.groups.contains(&g.name) && !g.services.contains(&s.name))
        {
            let mut n = self.node(
                "service",
                Some(&object.name),
                format!("service {}", object.name),
                Some(&object.origin),
            );
            n.detail = Some(format!("listed on service {}", object.name));
            services.push(n);
        }
        root.children.extend(self.section("services", services));
        // Hosts and host groups that get the group's services.
        let prefix = format!("servicegroup {name} on ");
        let mut listers: Vec<Node> = Vec::new();
        for s in &config.services {
            for a in &s.assigned {
                if let Reason::Listed { by, origin } = &a.reason
                    && let Some(on) = by.strip_prefix(&prefix)
                    && !listers.iter().any(|l| l.label == on)
                {
                    let (kind, target) = on.split_once(' ').unwrap_or(("host", on));
                    let kind = if kind == "hostgroup" {
                        "hostgroup"
                    } else {
                        "host"
                    };
                    listers.push(self.node(kind, Some(target), on, Some(origin)));
                }
            }
        }
        root.children.extend(self.section("assigned to", listers));
        root.children.extend(
            self.section(
                "members",
                config
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

    fn downtimegroup(&self, name: &str) -> Option<Node> {
        let config = self.config;
        let g = config.downtimegroups.iter().find(|g| g.name == name)?;
        let mut root = self.node(
            "downtimegroup",
            Some(&g.name),
            g.title.as_deref().unwrap_or(&g.name),
            Some(&g.origin),
        );
        let windows: Vec<Node> = g
            .schedule
            .iter()
            .map(|w| {
                let mut n = self.node("value", None, "window", Some(&g.origin));
                n.detail = Some(w.to_string());
                n
            })
            .collect();
        root.children.extend(self.section("schedule", windows));
        let members = config
            .hosts
            .iter()
            .filter_map(|h| {
                let why = match g.hostgroups.iter().find(|x| h.in_group(x)) {
                    Some(hg) => format!("in hostgroup {hg}"),
                    None => {
                        let m = g
                            .members
                            .iter()
                            .find(|m| crate::select::Glob::new(m).is_match(&h.name))?;
                        match m == &h.name {
                            true => "listed".to_string(),
                            false => format!("matched: members {m}"),
                        }
                    }
                };
                let mut n = self.node(
                    "host",
                    Some(&h.name),
                    format!("host {}", h.name),
                    Some(&h.origin),
                );
                n.detail = Some(why);
                Some(n)
            })
            .collect();
        root.children.extend(self.section("members", members));
        Some(root)
    }

    /// The name, or `@file:line` for an unnamed rule, as the editor
    /// addresses it.
    fn rule_id(&self, rule: &NotifyRule) -> String {
        match &rule.name {
            Some(name) => name.clone(),
            None => format!("@{}:{}", self.file(&rule.origin), rule.origin.line),
        }
    }

    fn rule_link(&self, rule: &NotifyRule) -> Node {
        self.node(
            "notify",
            Some(&self.rule_id(rule)),
            format!("notify {}", rule.label()),
            Some(&rule.origin),
        )
    }

    fn rule(&self, id: &str) -> Option<Node> {
        let config = self.config;
        let rule = config.notify.iter().find(|r| self.rule_id(r) == id)?;
        let mut root = self.rule_link(rule);
        root.label = rule.label();
        root.children
            .extend(self.section("to", self.rule_targets(rule)));
        let mut matched = Vec::new();
        for s in &config.services {
            let host_alert = config.is_host_check(s);
            if let Some((_, why)) = config
                .matching_rules(s, host_alert)
                .into_iter()
                .find(|(r, _)| std::ptr::eq(*r, rule))
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

[service.disk]
interval = "2m"
for_each = "disks"

[servicegroup.basics]
services = ["disk"]

[hostgroup.linux]
members = ["db*"]
servicegroups = ["basics"]

[contact.ops]
email = "ops@example.com"

[[notify]]
contacts = ["ops"]
hosts = { groups = ["linux"] }

[[host]]
name = "db01"
vars.disks.data = { path = "/srv", warn = "5%" }
"#,
        )
        .unwrap();
        let config = load_dir(&dir).unwrap().config;
        let builder = Builder::new(&config, Some(&dir));
        let text = render(&builder.tree("service", "db01/disk_data").unwrap());
        for expected in [
            "db01/disk_data",
            "service disk",
            "servicegroup basics on hostgroup linux: listed",
            "check disk",
            "interval: 2m",
            "vars.warn: 5%",
            "host db01, vars.disks.data",
            "notify rule at",
            "matched: host: group linux",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in\n{text}");
        }
        assert!(text.contains("a.toml:5"), "{text}");

        // Unnamed rules: the editor's id, `@file:line`.
        let (objects, _) = crate::edit::objects(&dir).unwrap();
        let rule = objects
            .iter()
            .find(|o| o.kind == crate::edit::Kind::Notify)
            .unwrap();
        assert_eq!(rule.id(), "@a.toml:19");
        let tree = builder.tree("service", "db01/disk_data").unwrap();
        fn names(n: &Node, out: &mut Vec<String>) {
            out.extend(n.name.clone());
            n.children.iter().for_each(|c| names(c, out));
        }
        let mut found = Vec::new();
        names(&tree, &mut found);
        assert!(found.contains(&rule.id()), "{found:?}");
        let text = render(&builder.tree("notify", &rule.id()).unwrap());
        assert!(text.contains("service db01/disk_data"), "{text}");

        let text = render(&builder.tree("service", "disk").unwrap());
        for expected in [
            "check disk",
            "servicegroup basics",
            "service db01/disk_data",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in\n{text}");
        }
        let text = render(&builder.tree("host", "db01").unwrap());
        assert!(
            text.contains("hostgroup linux: matched: members db*"),
            "{text}"
        );
        assert!(
            text.contains(
                "service disk_data: servicegroup basics on hostgroup linux; for_each disks.data"
            ),
            "{text}"
        );
        let text = render(&builder.tree("servicegroup", "basics").unwrap());
        assert!(text.contains("hostgroup linux"), "{text}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
