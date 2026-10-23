//! Selectors: which hosts or services a notify rule applies to.
//!
//! A selector is a table; every field present must match (AND), a list
//! matches any of its values (OR). `exclude` removes matches, `expr` is an
//! expression for what the fields can't say. A plain list is shorthand for
//! names. Matching returns why, e.g. `group prod, node dc1`, for the Config
//! Tree.

use crate::expr::{Expr, Subject};
use crate::{Host, Service};
use regex::Regex;
use std::collections::BTreeMap;
use toml::Value;

/// A name pattern with `*` and `?`.
#[derive(Debug, Clone)]
pub(crate) struct Glob {
    pub(crate) text: String,
    re: Regex,
}

impl Glob {
    pub(crate) fn new(text: &str) -> Self {
        let mut pattern = String::from("^");
        for c in text.chars() {
            match c {
                '*' => pattern.push_str(".*"),
                '?' => pattern.push('.'),
                c => pattern.push_str(&regex::escape(&c.to_string())),
            }
        }
        pattern.push('$');
        let re = Regex::new(&pattern).expect("escaped glob is a valid regex");
        Glob {
            text: text.to_string(),
            re,
        }
    }

    pub(crate) fn is_match(&self, s: &str) -> bool {
        self.re.is_match(s)
    }

    pub(crate) fn is_pattern(&self) -> bool {
        self.text.contains(['*', '?'])
    }
}

#[derive(Debug, Clone, Default)]
pub struct HostSelector {
    names: Vec<Glob>,
    addresses: Vec<Glob>,
    nodes: Vec<String>,
    groups: Vec<String>,
    vars: BTreeMap<String, Vec<String>>,
    exclude: Option<Box<HostSelector>>,
    expr: Option<Expr>,
}

/// What a selector refers to, for warnings about typos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ref {
    Host,
    Node,
    HostGroup,
    Check,
    ServiceGroup,
}

impl HostSelector {
    /// Parses a selector: a list of host names (globs), or a table.
    pub fn parse(value: &Value) -> Result<Self, String> {
        let mut sel = HostSelector::default();
        let table = match value {
            Value::Array(_) => {
                sel.names = strings(value, "hosts")?
                    .iter()
                    .map(|s| Glob::new(s))
                    .collect();
                return Ok(sel);
            }
            Value::Table(t) => t,
            _ => return Err("expected a list of host names or a table".into()),
        };
        for (key, v) in table {
            match key.as_str() {
                "name" => sel.names = strings(v, key)?.iter().map(|s| Glob::new(s)).collect(),
                "address" => {
                    sel.addresses = strings(v, key)?.iter().map(|s| Glob::new(s)).collect()
                }
                "node" => sel.nodes = strings(v, key)?,
                "groups" => sel.groups = strings(v, key)?,
                "vars" => {
                    let Value::Table(vars) = v else {
                        return Err("vars: expected a table".into());
                    };
                    for (name, values) in vars {
                        sel.vars
                            .insert(name.clone(), texts(values, &format!("vars.{name}"))?);
                    }
                }
                "exclude" => {
                    let ex = HostSelector::parse(v).map_err(|e| format!("exclude: {e}"))?;
                    sel.exclude = Some(Box::new(ex));
                }
                "expr" => {
                    let Value::String(src) = v else {
                        return Err("expr: expected a string".into());
                    };
                    let expr = Expr::parse(src).map_err(|e| format!("expr: {e}"))?;
                    if expr.uses_service() {
                        return Err("expr: service.* is not available for hosts".into());
                    }
                    sel.expr = Some(expr);
                }
                other => {
                    return Err(format!(
                        "unknown field {other:?} (expected name, address, node, groups, vars, \
                         exclude or expr)"
                    ));
                }
            }
        }
        Ok(sel)
    }

    /// Why `host` matches, or `None`.
    pub fn matches(&self, host: &Host) -> Option<String> {
        let mut why = Vec::new();
        if !self.names.is_empty() {
            let g = self.names.iter().find(|g| g.is_match(&host.name))?;
            why.push(match g.is_pattern() {
                true => format!("name {}", g.text),
                false => "listed".to_string(),
            });
        }
        if !self.addresses.is_empty() {
            let g = self.addresses.iter().find(|g| g.is_match(&host.address))?;
            why.push(format!("address {}", g.text));
        }
        if !self.nodes.is_empty() {
            let n = self.nodes.iter().find(|n| host.node.as_ref() == Some(n))?;
            why.push(format!("node {n}"));
        }
        if !self.groups.is_empty() {
            let g = self.groups.iter().find(|g| host.in_group(g))?;
            why.push(format!("group {g}"));
        }
        for (name, values) in &self.vars {
            let value = host.vars.get(name).and_then(|v| v.as_text())?;
            if !values.iter().any(|v| v == value) {
                return None;
            }
            why.push(format!("vars.{name} = {value}"));
        }
        if let Some(expr) = &self.expr {
            if !expr.matches(&Subject {
                host,
                service: None,
            }) {
                return None;
            }
            why.push("expr".into());
        }
        if let Some(ex) = &self.exclude
            && ex.matches(host).is_some()
        {
            return None;
        }
        Some(match why.is_empty() {
            true => "all hosts".into(),
            false => why.join(", "),
        })
    }

    /// Calls `f` with every name the selector refers to.
    pub(crate) fn refs(&self, f: &mut impl FnMut(Ref, &str)) {
        for g in self.names.iter().filter(|g| !g.is_pattern()) {
            f(Ref::Host, &g.text);
        }
        self.nodes.iter().for_each(|n| f(Ref::Node, n));
        self.groups.iter().for_each(|g| f(Ref::HostGroup, g));
        if let Some(ex) = &self.exclude {
            ex.refs(f);
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ServiceSelector {
    /// The shorthand list: `host/service` ids and service names.
    listed: Vec<(Option<Glob>, Glob)>,
    names: Vec<Glob>,
    checks: Vec<String>,
    groups: Vec<String>,
    hosts: Option<HostSelector>,
    exclude: Option<Box<ServiceSelector>>,
    expr: Option<Expr>,
}

impl ServiceSelector {
    /// Parses a selector: a list of `host/service` ids and service names
    /// (globs), or a table.
    pub fn parse(value: &Value) -> Result<Self, String> {
        let mut sel = ServiceSelector::default();
        let table = match value {
            Value::Array(_) => {
                for item in strings(value, "services")? {
                    sel.listed.push(match item.split_once('/') {
                        Some((h, s)) => (Some(Glob::new(h)), Glob::new(s)),
                        None => (None, Glob::new(&item)),
                    });
                }
                return Ok(sel);
            }
            Value::Table(t) => t,
            _ => return Err("expected a list of services or a table".into()),
        };
        for (key, v) in table {
            match key.as_str() {
                "name" => sel.names = strings(v, key)?.iter().map(|s| Glob::new(s)).collect(),
                "checks" => sel.checks = strings(v, key)?,
                "groups" => sel.groups = strings(v, key)?,
                "hosts" => {
                    sel.hosts = Some(HostSelector::parse(v).map_err(|e| format!("hosts: {e}"))?)
                }
                "exclude" => {
                    let ex = ServiceSelector::parse(v).map_err(|e| format!("exclude: {e}"))?;
                    sel.exclude = Some(Box::new(ex));
                }
                "expr" => {
                    let Value::String(src) = v else {
                        return Err("expr: expected a string".into());
                    };
                    sel.expr = Some(Expr::parse(src).map_err(|e| format!("expr: {e}"))?);
                }
                other => {
                    return Err(format!(
                        "unknown field {other:?} (expected name, checks, groups, hosts, \
                         exclude or expr)"
                    ));
                }
            }
        }
        Ok(sel)
    }

    /// Why `service` (on `host`) matches, or `None`.
    pub fn matches(&self, service: &Service, host: &Host) -> Option<String> {
        let mut why = Vec::new();
        if !self.listed.is_empty() {
            self.listed.iter().find(|(h, s)| {
                h.as_ref().is_none_or(|h| h.is_match(&host.name)) && s.is_match(&service.name)
            })?;
            why.push("listed".to_string());
        }
        if !self.names.is_empty() {
            let g = self.names.iter().find(|g| g.is_match(&service.name))?;
            why.push(format!("name {}", g.text));
        }
        if !self.checks.is_empty() {
            let c = self
                .checks
                .iter()
                .find(|c| service.check.as_ref() == Some(c))?;
            why.push(format!("check {c}"));
        }
        if !self.groups.is_empty() {
            let g = self.groups.iter().find(|g| service.in_group(g))?;
            why.push(format!("group {g}"));
        }
        if let Some(hosts) = &self.hosts {
            why.push(format!("host {}", hosts.matches(host)?));
        }
        if let Some(expr) = &self.expr {
            let subject = Subject {
                host,
                service: Some(service),
            };
            if !expr.matches(&subject) {
                return None;
            }
            why.push("expr".into());
        }
        if let Some(ex) = &self.exclude
            && ex.matches(service, host).is_some()
        {
            return None;
        }
        Some(match why.is_empty() {
            true => "all services".into(),
            false => why.join(", "),
        })
    }

    /// Calls `f` with every name the selector refers to.
    pub(crate) fn refs(&self, f: &mut impl FnMut(Ref, &str)) {
        self.checks.iter().for_each(|c| f(Ref::Check, c));
        self.groups.iter().for_each(|g| f(Ref::ServiceGroup, g));
        if let Some(hosts) = &self.hosts {
            hosts.refs(f);
        }
        if let Some(ex) = &self.exclude {
            ex.refs(f);
        }
    }
}

/// A string or a list of strings.
fn strings(v: &Value, key: &str) -> Result<Vec<String>, String> {
    match v {
        Value::String(s) => Ok(vec![s.clone()]),
        Value::Array(items) => items
            .iter()
            .map(|i| match i {
                Value::String(s) => Ok(s.clone()),
                _ => Err(format!("{key}: expected strings")),
            })
            .collect(),
        _ => Err(format!("{key}: expected a string or a list of strings")),
    }
}

/// Like [`strings`], with numbers and booleans as text (var values).
fn texts(v: &Value, key: &str) -> Result<Vec<String>, String> {
    let one = |v: &Value| match v {
        Value::String(s) => Ok(s.clone()),
        Value::Integer(n) => Ok(n.to_string()),
        Value::Float(n) => Ok(n.to_string()),
        Value::Boolean(b) => Ok(b.to_string()),
        _ => Err(format!("{key}: expected a value or a list of values")),
    };
    match v {
        Value::Array(items) => items.iter().map(one).collect(),
        v => Ok(vec![one(v)?]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Assigned, Reason, Var};

    fn host(name: &str, groups: &[&str]) -> Host {
        Host {
            name: name.into(),
            address: format!("{name}.example"),
            node: Some("dc1".into()),
            groups: groups
                .iter()
                .map(|g| Assigned {
                    name: g.to_string(),
                    reason: Reason::Listed {
                        by: "x".into(),
                        origin: Default::default(),
                    },
                })
                .collect(),
            vars: [("os".to_string(), Var::Text("debian".into()))].into(),
            host_check: None,
            graph: None,
            enabled: true,
            origin: Default::default(),
            sources: Default::default(),
        }
    }

    fn sel(toml: &str) -> HostSelector {
        let v: toml::Table = toml::from_str(&format!("s = {toml}")).unwrap();
        HostSelector::parse(&v["s"]).unwrap()
    }

    #[test]
    fn host_fields_and_together() {
        let web = host("web01", &["web", "prod"]);
        assert_eq!(
            sel(r#"{ groups = ["db", "prod"] }"#)
                .matches(&web)
                .as_deref(),
            Some("group prod")
        );
        assert_eq!(
            sel(r#"{ name = "web*", node = "dc1" }"#)
                .matches(&web)
                .as_deref(),
            Some("name web*, node dc1")
        );
        assert_eq!(
            sel(r#"["db01", "web01"]"#).matches(&web).as_deref(),
            Some("listed")
        );
        assert_eq!(
            sel(r#"{ vars.os = ["debian", "ubuntu"] }"#)
                .matches(&web)
                .as_deref(),
            Some("vars.os = debian")
        );
        assert_eq!(sel("{}").matches(&web).as_deref(), Some("all hosts"));
        assert!(
            sel(r#"{ groups = ["web"], node = "dc2" }"#)
                .matches(&web)
                .is_none()
        );
        assert!(
            sel(r#"{ groups = ["web"], exclude = { name = "web0?" } }"#)
                .matches(&web)
                .is_none()
        );
        assert!(sel(r#"{ vars.missing = "x" }"#).matches(&web).is_none());
        assert_eq!(
            sel(r#"{ expr = 'host.groups contains "web" or host.node == "dc9"' }"#)
                .matches(&web)
                .as_deref(),
            Some("expr")
        );
    }

    #[test]
    fn parse_errors() {
        let err = |toml: &str| {
            let v: toml::Table = toml::from_str(&format!("s = {toml}")).unwrap();
            HostSelector::parse(&v["s"]).unwrap_err()
        };
        assert!(err(r#"{ tags = ["x"] }"#).contains("unknown field \"tags\""));
        assert!(err("42").contains("expected a list"));
        assert!(err(r#"{ expr = 'service.name == "x"' }"#).contains("not available"));
        assert!(err(r#"{ expr = 'host.nme == "x"' }"#).contains("expr:"));
    }
}
