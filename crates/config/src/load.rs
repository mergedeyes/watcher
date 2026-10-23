//! Loading the object files into a [`Config`].
//!
//! A host gets the services listed on it and on its host groups, directly
//! or in a service group. Every value of a service is resolved layer by
//! layer, least specific first: built-in defaults, the check, the service,
//! the host's vars (over those of its host groups), the `for_each` entry
//! and the host's own `service.<name>`. Layers are merged as TOML tables,
//! recording which layer set each key; that record is what the Config Tree
//! shows.
//!
//! Host groups of the same host are the same level: if two of them set a
//! key to different values, that is an error naming both, never a silent
//! pick.

use crate::select::{Glob, HostSelector, Ref, ServiceSelector};
use crate::{
    Assigned, Check, Config, Contact, Diagnostic, Group, Host, Layer, Loaded, NotifyOn, NotifyRule,
    Origin, Reason, Service, ServiceObject, Settings, Severity, Source, Var, Vars, macros, var_at,
};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use toml::{Spanned, Table, Value};
use watcher_core::{FlapThresholds, Format, ThresholdSpec};
use watcher_exec::CommandLine;

/// Loads and compiles all `*.toml` files below `dir`.
///
/// Returns the config and its warnings, or every problem found (errors and
/// warnings) if there is at least one error.
pub fn load_dir(dir: &Path) -> Result<Loaded, Vec<Diagnostic>> {
    load_dir_with(dir, &BTreeMap::new())
}

/// Like [`load_dir`], with some files replaced: `overlay` maps a path below
/// `dir` to its new text, or to `None` for a deleted file. Paths that don't
/// exist yet are added. Nothing is written.
pub fn load_dir_with(
    dir: &Path,
    overlay: &BTreeMap<PathBuf, Option<String>>,
) -> Result<Loaded, Vec<Diagnostic>> {
    let mut sink = Sink::default();
    let mut raw = RawObjects::default();
    let mut files = match discover(dir) {
        Ok(files) => files,
        Err(err) => {
            sink.error(None, format!("cannot read {}: {err}", dir.display()));
            return Err(sink.finish());
        }
    };
    files.extend(overlay.keys().cloned());
    files.retain(|f| !matches!(overlay.get(f), Some(None)));
    files.sort();
    files.dedup();
    for file in &files {
        match overlay.get(file) {
            Some(Some(text)) => raw.add_file(file, text, &mut sink),
            _ => match std::fs::read_to_string(file) {
                Ok(text) => raw.add_file(file, &text, &mut sink),
                Err(err) => sink.error(None, format!("cannot read {}: {err}", file.display())),
            },
        }
    }
    let mut config = raw.compile(&mut sink);
    config.files = files;
    if sink.has_errors() {
        Err(sink.finish())
    } else {
        Ok(Loaded {
            config,
            warnings: sink.finish(),
        })
    }
}

/// All `*.toml` files below `dir`, sorted, skipping hidden entries.
pub(crate) fn discover(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if entry.file_type()?.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|e| e == "toml") {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

#[derive(Default)]
struct Sink {
    diagnostics: Vec<Diagnostic>,
}

impl Sink {
    fn error(&mut self, origin: Option<&Origin>, message: impl Into<String>) {
        self.push(origin, Severity::Error, message.into());
    }

    fn warning(&mut self, origin: Option<&Origin>, message: impl Into<String>) {
        self.push(origin, Severity::Warning, message.into());
    }

    fn push(&mut self, origin: Option<&Origin>, severity: Severity, message: String) {
        self.diagnostics.push(Diagnostic {
            origin: origin.cloned(),
            severity,
            message,
        });
    }

    fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }

    /// Sorted by file and line; a group problem hit by several hosts is
    /// reported once.
    fn finish(mut self) -> Vec<Diagnostic> {
        self.diagnostics.sort();
        self.diagnostics.dedup();
        self.diagnostics
    }
}

/// The layout of an object file.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawFile {
    #[serde(default)]
    pub check: BTreeMap<String, Spanned<Table>>,
    #[serde(default)]
    pub service: BTreeMap<String, Spanned<Table>>,
    #[serde(default)]
    pub host: Vec<Spanned<Table>>,
    #[serde(default)]
    pub hostgroup: BTreeMap<String, Spanned<Table>>,
    #[serde(default)]
    pub servicegroup: BTreeMap<String, Spanned<Table>>,
    #[serde(default)]
    pub contact: BTreeMap<String, Spanned<Table>>,
    #[serde(default)]
    pub contactgroup: BTreeMap<String, Spanned<Table>>,
    #[serde(default)]
    pub downtimegroup: BTreeMap<String, Spanned<Table>>,
    #[serde(default)]
    pub notify: Vec<Spanned<Table>>,
    #[serde(default)]
    pub sink: Vec<Spanned<Table>>,
}

/// Where the `service.<name>` entries of hosts are, read separately since
/// they sit inside the hosts.
#[derive(Default, Deserialize)]
struct Spans {
    #[serde(default)]
    host: Vec<ServiceSpans>,
}

#[derive(Default, Deserialize)]
struct ServiceSpans {
    #[serde(default)]
    service: BTreeMap<String, Spanned<Value>>,
}

/// Parses an object file.
pub(crate) fn parse_file(text: &str) -> Result<RawFile, toml::de::Error> {
    toml::from_str(text)
}

/// A hint when `text` uses an older layout, which is no longer read.
fn old_format_hint(text: &str) -> Option<&'static str> {
    let table: Table = toml::from_str(text).ok()?;
    if table.contains_key("role") {
        return Some(
            "roles are gone: define [service.<name>] and assign it with services = [...] or \
             servicegroups = [...] on a host or host group",
        );
    }
    if table.contains_key("template") {
        return Some("templates are gone: define [service.<name>] and assign it to hosts");
    }
    if matches!(table.get("check"), Some(Value::Array(_))) {
        return Some(
            "[[check]] is the old format: write [check.<name>] (what to run) and \
             [service.<name>] (assigned to hosts)",
        );
    }
    None
}

struct RawObject {
    name: String,
    table: Table,
    origin: Origin,
    /// Lines of the `service.<name>` entries (hosts).
    services: BTreeMap<String, Origin>,
}

#[derive(Default)]
struct RawObjects {
    checks: Vec<RawObject>,
    services: Vec<RawObject>,
    hosts: Vec<RawObject>,
    hostgroups: Vec<RawObject>,
    servicegroups: Vec<RawObject>,
    contacts: Vec<RawObject>,
    contactgroups: Vec<RawObject>,
    downtimegroups: Vec<RawObject>,
    notify: Vec<RawObject>,
    sinks: Vec<RawObject>,
}

impl RawObjects {
    fn add_file(&mut self, path: &Path, text: &str, sink: &mut Sink) {
        let origin = |offset: usize| Origin {
            file: path.to_path_buf(),
            line: text[..offset.min(text.len())].matches('\n').count() + 1,
        };
        let file = match parse_file(text) {
            Ok(file) => file,
            Err(err) => {
                let at = err.span().map_or(
                    Origin {
                        file: path.into(),
                        line: 0,
                    },
                    |s| origin(s.start),
                );
                let message = match old_format_hint(text) {
                    Some(hint) => format!("{} ({hint})", err.message().trim()),
                    None => err.message().trim().to_string(),
                };
                sink.error(Some(&at), message);
                return;
            }
        };
        let spans: Spans = toml::from_str(text).unwrap_or_default();
        let keyed = |list: &mut Vec<RawObject>, map: BTreeMap<String, Spanned<Table>>| {
            for (name, t) in map {
                list.push(RawObject {
                    name,
                    origin: origin(t.span().start),
                    table: t.into_inner(),
                    services: BTreeMap::new(),
                });
            }
        };
        keyed(&mut self.checks, file.check);
        keyed(&mut self.services, file.service);
        keyed(&mut self.hostgroups, file.hostgroup);
        keyed(&mut self.servicegroups, file.servicegroup);
        keyed(&mut self.contacts, file.contact);
        keyed(&mut self.contactgroups, file.contactgroup);
        keyed(&mut self.downtimegroups, file.downtimegroup);
        for (i, t) in file.host.into_iter().enumerate() {
            let table = t.get_ref();
            let name = table
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let services = spans
                .host
                .get(i)
                .map(|s| {
                    s.service
                        .iter()
                        .map(|(name, v)| (name.clone(), origin(v.span().start)))
                        .collect()
                })
                .unwrap_or_default();
            self.hosts.push(RawObject {
                name: name.to_string(),
                services,
                origin: origin(t.span().start),
                table: t.into_inner(),
            });
        }
        let named = |list: &mut Vec<RawObject>, items: Vec<Spanned<Table>>| {
            for t in items {
                let table = t.get_ref();
                let name = table
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                list.push(RawObject {
                    name: name.to_string(),
                    origin: origin(t.span().start),
                    table: t.into_inner(),
                    services: BTreeMap::new(),
                });
            }
        };
        named(&mut self.notify, file.notify);
        named(&mut self.sinks, file.sink);
    }

    fn compile(&self, sink: &mut Sink) -> Config {
        let mut config = Config::default();
        let mut c = Compiler {
            raw: self,
            sink,
            checks: BTreeMap::new(),
            objects: BTreeMap::new(),
            sg_services: BTreeMap::new(),
            hostgroups: BTreeMap::new(),
        };

        c.compile_checks(&mut config);
        c.compile_service_objects(&mut config);
        c.compile_servicegroups(&mut config);
        c.compile_hostgroups(&mut config);
        let hosts = c.compile_hosts(&mut config);
        c.compile_services(&mut config, &hosts);
        c.compile_downtimegroups(&mut config);
        c.compile_contacts(&mut config);
        c.compile_notify(&mut config);
        c.compile_sinks(&mut config);
        c.check_references(&config);
        config
    }
}

/// A table merged from layers, with the source of every key (by dotted
/// path).
#[derive(Debug, Clone, Default, PartialEq)]
struct Layered {
    table: Table,
    sources: BTreeMap<String, Source>,
}

impl Layered {
    /// Merges `table` over what is there: tables merge per key, everything
    /// else is replaced, except `groups`, whose lists add up.
    fn add(&mut self, table: &Table, source: &Source) {
        add_into(&mut self.table, &mut self.sources, "", table, source);
    }

    fn with(mut self, table: &Table, source: &Source) -> Self {
        self.add(table, source);
        self
    }

    /// Merges layers of the same level. A key set by two of them to
    /// different values is a conflict: `(key, source a, source b)`.
    /// `groups` lists add up instead.
    fn siblings(parts: &[&Layered]) -> (Layered, Vec<(String, Source, Source)>) {
        let mut out = Layered::default();
        let mut conflicts = Vec::new();
        for part in parts {
            for (path, value) in leaves(&part.table, "") {
                let Some(src) = part.sources.get(&path) else {
                    continue;
                };
                let mut single = Table::new();
                set_path(&mut single, &path, value.clone());
                match get_path(&out.table, &path) {
                    Some(existing) if path != "groups" && existing != value => {
                        if let Some(prev) = out.sources.get(&path) {
                            conflicts.push((path.clone(), prev.clone(), src.clone()));
                        }
                    }
                    Some(_) if path != "groups" => {}
                    _ => out.add(&single, src),
                }
            }
        }
        (out, conflicts)
    }
}

fn add_into(
    into: &mut Table,
    sources: &mut BTreeMap<String, Source>,
    prefix: &str,
    from: &Table,
    source: &Source,
) {
    for (key, value) in from {
        let path = match prefix {
            "" => key.clone(),
            p => format!("{p}.{key}"),
        };
        match (into.get_mut(key), value) {
            (Some(Value::Table(a)), Value::Table(b)) => add_into(a, sources, &path, b, source),
            (Some(Value::Array(a)), Value::Array(b)) if key == "groups" => {
                for item in b {
                    if !a.contains(item) {
                        a.push(item.clone());
                    }
                }
            }
            (_, Value::Table(b)) => {
                sources.retain(|p, _| !p.starts_with(&format!("{path}.")) && *p != path);
                let mut t = Table::new();
                add_into(&mut t, sources, &path, b, source);
                into.insert(key.clone(), Value::Table(t));
                // An empty table is a value too (a `for_each` entry `{}`).
                if b.is_empty() {
                    sources.insert(path, source.clone());
                }
            }
            (_, value) => {
                sources.retain(|p, _| !p.starts_with(&format!("{path}.")));
                into.insert(key.clone(), value.clone());
                sources.insert(path, source.clone());
            }
        }
    }
}

/// Every non-table value with its dotted path.
fn leaves<'a>(table: &'a Table, prefix: &str) -> Vec<(String, &'a Value)> {
    let mut out = Vec::new();
    for (key, value) in table {
        let path = match prefix {
            "" => key.clone(),
            p => format!("{p}.{key}"),
        };
        match value {
            Value::Table(t) if !t.is_empty() => out.extend(leaves(t, &path)),
            v => out.push((path, v)),
        }
    }
    out
}

fn get_path<'a>(table: &'a Table, path: &str) -> Option<&'a Value> {
    let mut parts = path.split('.');
    let mut value = table.get(parts.next()?)?;
    for part in parts {
        value = value.as_table()?.get(part)?;
    }
    Some(value)
}

fn set_path(table: &mut Table, path: &str, value: Value) {
    let parts: Vec<&str> = path.split('.').collect();
    let (last, parents) = parts.split_last().expect("paths are not empty");
    let mut t = table;
    for part in parents {
        let entry = t
            .entry(part.to_string())
            .or_insert_with(|| Value::Table(Table::new()));
        if !entry.is_table() {
            *entry = Value::Table(Table::new());
        }
        t = entry.as_table_mut().expect("just made a table");
    }
    t.insert(last.to_string(), value);
}

/// Keys of a service that are not settings.
const META_KEYS: [&str; 5] = ["check", "for_each", "groups", "enabled", "title"];

/// The host settings of a host or host group as a table, for layering.
fn host_part(
    address: Option<&str>,
    node: Option<&str>,
    host_check: Option<&str>,
    graph: Option<&str>,
    vars: &Table,
) -> Table {
    let mut t = Table::new();
    for (key, value) in [
        ("address", address),
        ("node", node),
        ("host_check", host_check),
        ("graph", graph),
    ] {
        if let Some(v) = value {
            t.insert(key.into(), Value::String(v.into()));
        }
    }
    if !vars.is_empty() {
        t.insert("vars".into(), Value::Table(vars.clone()));
    }
    t
}

/// A host group, validated.
struct HostGroupInfo {
    def: HostGroupDef,
    origin: Origin,
    members: Vec<Glob>,
    /// What its hosts get: `host_check`, `graph`, `vars`.
    host: Layered,
}

struct Compiler<'a> {
    raw: &'a RawObjects,
    sink: &'a mut Sink,
    /// Check tables, validated, by name.
    checks: BTreeMap<String, (Table, Origin)>,
    /// Service object tables, validated, by name.
    objects: BTreeMap<String, (Table, Origin)>,
    /// The services of each service group: listed in the group or listing
    /// it, with why.
    sg_services: BTreeMap<String, Vec<(String, Reason)>>,
    hostgroups: BTreeMap<String, HostGroupInfo>,
}

/// A service a host will get, before its layers are resolved.
struct Planned {
    name: String,
    /// The service object or host service it comes from (for `for_each`,
    /// the object's name, not `<name>_<key>`).
    base: String,
    /// The service object it is made from.
    object: Option<String>,
    assigned: Vec<Assigned>,
    /// `(var, key, the entry's vars)` for `for_each` services.
    for_each: Option<(String, String, Table)>,
}

/// A host before its services: the raw object, its own service tables and
/// the service objects assigned to it.
struct HostInput<'a> {
    raw: &'a RawObject,
    services: BTreeMap<String, Table>,
    assigned: BTreeMap<String, Vec<Assigned>>,
}

impl<'a> Compiler<'a> {
    fn compile_checks(&mut self, config: &mut Config) {
        for raw in &self.raw.checks {
            if self.duplicate("check", &raw.name, &raw.origin, &self.raw.checks) {
                continue;
            }
            let what = format!("check {:?}", raw.name);
            if let Err(msg) = valid_name(&raw.name) {
                self.sink.error(Some(&raw.origin), format!("{what}: {msg}"));
                continue;
            }
            let mut ok = true;
            for key in raw.table.keys() {
                if ["check", "for_each", "groups", "enabled"].contains(&key.as_str()) {
                    self.sink.error(
                        Some(&raw.origin),
                        format!("{what}: {key} belongs on a service ([service.<name>])"),
                    );
                    ok = false;
                }
            }
            let Some(def) = self.settings(&raw.table, &raw.origin, &what) else {
                continue;
            };
            if !ok {
                continue;
            }
            if let Some(command) = &def.command {
                self.check_command(command, def.shell.unwrap_or(false), &raw.origin, &what);
            }
            self.checks
                .insert(raw.name.clone(), (raw.table.clone(), raw.origin.clone()));
            config.checks.push(Check {
                name: raw.name.clone(),
                title: def.title,
                command: def.command,
                origin: raw.origin.clone(),
            });
        }
    }

    fn compile_service_objects(&mut self, config: &mut Config) {
        for raw in &self.raw.services {
            if self.duplicate("service", &raw.name, &raw.origin, &self.raw.services) {
                continue;
            }
            let what = format!("service {:?}", raw.name);
            if let Err(msg) = valid_name(&raw.name) {
                self.sink.error(Some(&raw.origin), format!("{what}: {msg}"));
                continue;
            }
            let Some(def) = self.settings(&raw.table, &raw.origin, &what) else {
                continue;
            };
            if let Some(command) = &def.command {
                self.check_command(command, def.shell.unwrap_or(false), &raw.origin, &what);
            }
            let check = match &def.check {
                Some(c) if !self.checks.contains_key(c) => {
                    self.sink
                        .error(Some(&raw.origin), format!("{what}: unknown check {c:?}"));
                    continue;
                }
                Some(c) => Some(c.clone()),
                None if self.checks.contains_key(&raw.name) => Some(raw.name.clone()),
                None => None,
            };
            if check.is_none() && def.command.is_none() && def.passive != Some(true) {
                self.sink.error(
                    Some(&raw.origin),
                    format!(
                        "{what}: no check {:?}; set check, command or passive = true",
                        raw.name
                    ),
                );
                continue;
            }
            self.objects
                .insert(raw.name.clone(), (raw.table.clone(), raw.origin.clone()));
            config.service_objects.push(ServiceObject {
                name: raw.name.clone(),
                title: def.title,
                check,
                groups: def.groups,
                origin: raw.origin.clone(),
            });
        }
    }

    fn compile_servicegroups(&mut self, config: &mut Config) {
        for raw in &self.raw.servicegroups {
            if self.duplicate(
                "servicegroup",
                &raw.name,
                &raw.origin,
                &self.raw.servicegroups,
            ) {
                continue;
            }
            let what = format!("servicegroup {:?}", raw.name);
            if let Err(msg) = valid_name(&raw.name) {
                self.sink.error(Some(&raw.origin), format!("{what}: {msg}"));
                continue;
            }
            let def: ServiceGroupDef =
                match ServiceGroupDef::deserialize(Value::Table(raw.table.clone())) {
                    Ok(d) => d,
                    Err(err) => {
                        self.sink.error(
                            Some(&raw.origin),
                            format!("{what}: {}", err.message().trim()),
                        );
                        continue;
                    }
                };
            let mut members = Vec::new();
            for svc in &def.services {
                if self.known_service(svc, &raw.origin, &what) {
                    members.push((
                        svc.clone(),
                        Reason::Listed {
                            by: format!("servicegroup {}", raw.name),
                            origin: raw.origin.clone(),
                        },
                    ));
                }
            }
            self.sg_services.insert(raw.name.clone(), members);
            config.servicegroups.push(Group {
                name: raw.name.clone(),
                title: def.title,
                members: Vec::new(),
                services: def.services,
                servicegroups: Vec::new(),
                origin: raw.origin.clone(),
            });
        }
        // Service objects listing a group.
        for object in &config.service_objects {
            for g in &object.groups {
                match self.sg_services.get_mut(g) {
                    None => self.sink.error(
                        Some(&object.origin),
                        format!("service {:?}: unknown servicegroup {g:?}", object.name),
                    ),
                    Some(list) if list.iter().any(|(s, _)| *s == object.name) => {}
                    Some(list) => list.push((
                        object.name.clone(),
                        Reason::Listed {
                            by: format!("service {}", object.name),
                            origin: object.origin.clone(),
                        },
                    )),
                }
            }
        }
    }

    /// Whether `name` is a service object; reports it if not.
    fn known_service(&mut self, name: &str, origin: &Origin, what: &str) -> bool {
        if self.objects.contains_key(name) {
            return true;
        }
        let msg = match self.checks.contains_key(name) {
            true => format!(
                "{what}: unknown service {name:?} (there is a check {name:?}: add [service.{name}])"
            ),
            false => format!("{what}: unknown service {name:?}"),
        };
        self.sink.error(Some(origin), msg);
        false
    }

    /// Whether `name` is a service group; reports it if not.
    fn known_servicegroup(&mut self, name: &str, origin: &Origin, what: &str) -> bool {
        let known = self.sg_services.contains_key(name);
        if !known {
            self.sink.error(
                Some(origin),
                format!("{what}: unknown servicegroup {name:?}"),
            );
        }
        known
    }

    fn compile_hostgroups(&mut self, config: &mut Config) {
        for raw in &self.raw.hostgroups {
            if self.duplicate("hostgroup", &raw.name, &raw.origin, &self.raw.hostgroups) {
                continue;
            }
            let what = format!("hostgroup {:?}", raw.name);
            if let Err(msg) = valid_name(&raw.name) {
                self.sink.error(Some(&raw.origin), format!("{what}: {msg}"));
                continue;
            }
            let def: HostGroupDef = match HostGroupDef::deserialize(Value::Table(raw.table.clone()))
            {
                Ok(d) => d,
                Err(err) => {
                    self.sink.error(
                        Some(&raw.origin),
                        format!("{what}: {}", err.message().trim()),
                    );
                    continue;
                }
            };
            let mut ok = true;
            for svc in &def.services {
                ok &= self.known_service(svc, &raw.origin, &what);
            }
            for sg in &def.servicegroups {
                ok &= self.known_servicegroup(sg, &raw.origin, &what);
            }
            if let Err(msg) = vars_of(&def.vars) {
                self.sink.error(Some(&raw.origin), format!("{what}: {msg}"));
                ok = false;
            }
            config.hostgroups.push(Group {
                name: raw.name.clone(),
                title: def.title.clone(),
                members: def.members.clone(),
                services: def.services.clone(),
                servicegroups: def.servicegroups.clone(),
                origin: raw.origin.clone(),
            });
            if !ok {
                continue;
            }
            let source = Source {
                layer: Layer::HostGroup,
                name: raw.name.clone(),
                origin: raw.origin.clone(),
            };
            let host = Layered::default().with(
                &host_part(
                    None,
                    None,
                    def.host_check.as_deref(),
                    def.graph.as_deref(),
                    &def.vars,
                ),
                &source,
            );
            self.hostgroups.insert(
                raw.name.clone(),
                HostGroupInfo {
                    members: def.members.iter().map(|m| Glob::new(m)).collect(),
                    def,
                    origin: raw.origin.clone(),
                    host,
                },
            );
        }
    }

    fn report_conflicts(
        &mut self,
        origin: &Origin,
        what: &str,
        among: &str,
        conflicts: Vec<(String, Source, Source)>,
    ) -> bool {
        for (key, a, b) in &conflicts {
            self.sink.error(
                Some(origin),
                format!(
                    "{what}: {among} set {key} differently: {} ({}) and {} ({})",
                    a.name, a.origin, b.name, b.origin
                ),
            );
        }
        conflicts.is_empty()
    }

    fn compile_hosts(&mut self, config: &mut Config) -> Vec<HostInput<'a>> {
        let mut inputs = Vec::new();
        for raw in &self.raw.hosts {
            let what = format!("host {:?}", raw.name);
            let def: HostDef = match HostDef::deserialize(Value::Table(raw.table.clone())) {
                Ok(def) => def,
                Err(err) => {
                    let msg = err.message().trim();
                    let hint = match () {
                        _ if msg.contains("`tags`") => {
                            " (tags are gone: put hosts in host groups, groups = [...])"
                        }
                        _ if msg.contains("`roles`") => {
                            " (roles are gone: assign services = [...] or servicegroups = [...])"
                        }
                        _ => "",
                    };
                    self.sink
                        .error(Some(&raw.origin), format!("{what}: {msg}{hint}"));
                    continue;
                }
            };
            if let Err(msg) = valid_name(&def.name) {
                self.sink.error(Some(&raw.origin), format!("{what}: {msg}"));
                continue;
            }
            if self.duplicate("host", &def.name, &raw.origin, &self.raw.hosts) {
                continue;
            }
            if let Err(msg) = vars_of(&def.vars) {
                self.sink.error(Some(&raw.origin), format!("{what}: {msg}"));
                continue;
            }
            let host_src = Source {
                layer: Layer::Host,
                name: def.name.clone(),
                origin: raw.origin.clone(),
            };
            let listed = |by: String| Reason::Listed {
                by,
                origin: raw.origin.clone(),
            };

            // Host groups: listed on the host, or the group's members.
            let mut groups: Vec<Assigned> = Vec::new();
            let mut ok = true;
            for g in &def.groups {
                if !config.hostgroups.iter().any(|x| x.name == *g) {
                    self.sink.error(
                        Some(&raw.origin),
                        format!("{what}: unknown hostgroup {g:?}"),
                    );
                    ok = false;
                    continue;
                }
                groups.push(Assigned {
                    name: g.clone(),
                    reason: listed(format!("host {}", def.name)),
                });
            }
            for (name, info) in &self.hostgroups {
                if groups.iter().any(|a| a.name == *name) {
                    continue;
                }
                if let Some(glob) = info.members.iter().find(|m| m.is_match(&def.name)) {
                    let reason = match glob.is_pattern() {
                        true => Reason::Selector {
                            origin: info.origin.clone(),
                            matched: format!("members {}", glob.text),
                        },
                        false => Reason::Listed {
                            by: format!("hostgroup {name}"),
                            origin: info.origin.clone(),
                        },
                    };
                    groups.push(Assigned {
                        name: name.clone(),
                        reason,
                    });
                }
            }
            // A group with errors gives nothing; they are reported there.
            if groups
                .iter()
                .any(|g| !self.hostgroups.contains_key(&g.name))
            {
                ok = false;
            }

            // What the host groups set, side by side, then the host itself.
            let parts: Vec<&Layered> = groups
                .iter()
                .filter_map(|g| self.hostgroups.get(&g.name).map(|i| &i.host))
                .collect();
            let (mut layered, conflicts) = Layered::siblings(&parts);
            ok &= self.report_conflicts(&raw.origin, &what, "host groups", conflicts);
            layered.add(
                &host_part(
                    def.address.as_deref(),
                    def.node.as_deref(),
                    def.host_check.as_deref(),
                    def.graph.as_deref(),
                    &def.vars,
                ),
                &host_src,
            );
            let vars = match vars_of(
                layered
                    .table
                    .get("vars")
                    .and_then(Value::as_table)
                    .unwrap_or(&Table::new()),
            ) {
                Ok(vars) => vars,
                Err(msg) => {
                    self.sink.error(Some(&raw.origin), format!("{what}: {msg}"));
                    continue;
                }
            };

            // Services: listed on the host or its groups, directly or in a
            // service group.
            let mut assigned: BTreeMap<String, Vec<Assigned>> = BTreeMap::new();
            let mut add = |svc: &str, reason: Reason| {
                let list = assigned.entry(svc.to_string()).or_default();
                if !list.iter().any(|a| a.reason == reason) {
                    list.push(Assigned {
                        name: svc.to_string(),
                        reason,
                    });
                }
            };
            for svc in &def.services {
                if self.known_service(svc, &raw.origin, &what) {
                    add(svc, listed(format!("host {}", def.name)));
                } else {
                    ok = false;
                }
            }
            for sg in &def.servicegroups {
                if !self.known_servicegroup(sg, &raw.origin, &what) {
                    ok = false;
                    continue;
                }
                for (svc, _) in &self.sg_services[sg] {
                    add(
                        svc,
                        listed(format!("servicegroup {sg} on host {}", def.name)),
                    );
                }
            }
            for g in &groups {
                let Some(info) = self.hostgroups.get(&g.name) else {
                    continue;
                };
                let by_group = |by: String| Reason::Listed {
                    by,
                    origin: info.origin.clone(),
                };
                for svc in &info.def.services {
                    add(svc, by_group(format!("hostgroup {}", g.name)));
                }
                for sg in &info.def.servicegroups {
                    for (svc, _) in self.sg_services.get(sg).into_iter().flatten() {
                        add(
                            svc,
                            by_group(format!("servicegroup {sg} on hostgroup {}", g.name)),
                        );
                    }
                }
            }
            if !ok {
                continue;
            }

            let text = |key: &str| {
                layered
                    .table
                    .get(key)
                    .and_then(Value::as_str)
                    .map(String::from)
            };
            config.hosts.push(Host {
                name: def.name.clone(),
                address: def.address.clone().unwrap_or_else(|| def.name.clone()),
                node: text("node"),
                groups,
                vars,
                host_check: text("host_check"),
                graph: text("graph"),
                enabled: def.enabled,
                origin: raw.origin.clone(),
                sources: layered
                    .sources
                    .iter()
                    .filter(|(k, _)| *k != "address" || def.address.is_some())
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            });
            inputs.push(HostInput {
                raw,
                services: def.service.clone(),
                assigned,
            });
        }
        inputs
    }

    fn compile_services(&mut self, config: &mut Config, inputs: &[HostInput<'a>]) {
        for (host_idx, input) in inputs.iter().enumerate() {
            let host = config.hosts[host_idx].clone();
            if !host.enabled {
                continue;
            }
            let what = format!("host {:?}", host.name);
            for plan in self.plan(&host, input) {
                if config
                    .services
                    .iter()
                    .any(|s| s.host == host_idx && s.name == plan.name)
                {
                    self.sink.error(
                        Some(&host.origin),
                        format!("{what}: service {:?} is defined twice", plan.name),
                    );
                    continue;
                }
                if let Some(service) = self.service(config, host_idx, input, &plan) {
                    config.services.push(service);
                }
            }

            if let Some(hc) = &host.host_check
                && !config
                    .services
                    .iter()
                    .any(|s| s.host == host_idx && s.name == *hc)
            {
                let origin = host
                    .sources
                    .get("host_check")
                    .map_or(&host.origin, |s| &s.origin);
                self.sink.error(
                    Some(origin),
                    format!("{what}: host_check {hc:?} is not a service on it"),
                );
            }
            if let Some(graph) = &host.graph
                && !config
                    .services
                    .iter()
                    .any(|s| s.host == host_idx && s.name == *graph)
            {
                self.sink.warning(
                    Some(&host.origin),
                    format!("{what}: graph {graph:?} is not a service on it"),
                );
            }
        }
    }

    /// The services a host gets: the assigned service objects (`for_each`
    /// expanded), then its own services that override none of them.
    fn plan(&mut self, host: &Host, input: &HostInput) -> Vec<Planned> {
        let what = format!("host {:?}", host.name);
        let mut wanted: Vec<(String, Vec<Assigned>)> = input
            .assigned
            .iter()
            .map(|(s, a)| (s.clone(), a.clone()))
            .collect();
        // A host service named like a service object assigns it too.
        for name in input.services.keys() {
            if self.objects.contains_key(name) && !input.assigned.contains_key(name) {
                let origin = input
                    .raw
                    .services
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| host.origin.clone());
                wanted.push((
                    name.clone(),
                    vec![Assigned {
                        name: name.clone(),
                        reason: Reason::Listed {
                            by: format!("host {}, service.{name}", host.name),
                            origin,
                        },
                    }],
                ));
            }
        }

        let mut planned = Vec::new();
        for (svc, assigned) in wanted {
            let Some((table, _)) = self.objects.get(&svc) else {
                continue;
            };
            let for_each = SettingsDef::deserialize(Value::Table(table.clone()))
                .ok()
                .and_then(|d| d.for_each);
            let Some(var) = for_each else {
                planned.push(Planned {
                    name: svc.clone(),
                    base: svc.clone(),
                    object: Some(svc),
                    assigned,
                    for_each: None,
                });
                continue;
            };
            match var_at(&host.vars, &var) {
                None => self.sink.warning(
                    Some(&host.origin),
                    format!("{what}: service {svc:?}: for_each: no host var {var:?}"),
                ),
                Some(Var::Text(_)) => self.sink.error(
                    Some(&host.origin),
                    format!("{what}: for_each {var:?}: vars.{var} must be a table of tables"),
                ),
                Some(Var::Table(items)) => {
                    for (key, item) in items {
                        let Var::Table(fields) = item else {
                            self.sink.error(
                                Some(&host.origin),
                                format!("{what}: vars.{var}.{key} must be a table"),
                            );
                            continue;
                        };
                        if let Err(msg) = valid_name(key) {
                            self.sink.error(
                                Some(&host.origin),
                                format!("{what}: vars.{var}.{key}: {msg}"),
                            );
                            continue;
                        }
                        let vars: Table = fields
                            .iter()
                            .filter_map(|(k, v)| {
                                Some((k.clone(), Value::String(v.as_text()?.to_string())))
                            })
                            .collect();
                        planned.push(Planned {
                            name: format!("{svc}_{key}"),
                            base: svc.clone(),
                            object: Some(svc.clone()),
                            assigned: assigned.clone(),
                            for_each: Some((var.clone(), key.clone(), vars)),
                        });
                    }
                }
            }
        }
        for name in input.services.keys() {
            if !planned.iter().any(|p| p.name == *name || p.base == *name) {
                planned.push(Planned {
                    name: name.clone(),
                    base: name.clone(),
                    object: None,
                    assigned: Vec::new(),
                    for_each: None,
                });
            }
        }
        planned
    }

    /// Resolves one planned service of a host from its layers.
    fn service(
        &mut self,
        config: &Config,
        host_idx: usize,
        input: &HostInput,
        plan: &Planned,
    ) -> Option<Service> {
        let host = &config.hosts[host_idx];
        let name = plan.name.as_str();
        let what = format!("host {:?}: service {name:?}", host.name);
        let host_src = Source {
            layer: Layer::Host,
            name: host.name.clone(),
            origin: host.origin.clone(),
        };
        let object = plan.object.as_ref().and_then(|o| {
            let (table, origin) = self.objects.get(o)?;
            let src = Source {
                layer: Layer::Service,
                name: o.clone(),
                origin: origin.clone(),
            };
            Some(Layered::default().with(table, &src))
        });

        // The host's overrides: for a whole `for_each` service, then this one.
        let mut overrides: Vec<(Table, Source)> = Vec::new();
        let mut keys = vec![&plan.base];
        if plan.base != plan.name {
            keys.push(&plan.name);
        }
        for key in keys {
            let Some(table) = input.services.get(key) else {
                continue;
            };
            let o = input
                .raw
                .services
                .get(key)
                .cloned()
                .unwrap_or_else(|| host.origin.clone());
            self.settings(table, &o, &what)?;
            overrides.push((
                table.clone(),
                Source {
                    layer: Layer::HostService,
                    name: format!("{}.{key}", host.name),
                    origin: o,
                },
            ));
        }
        let object_origin = plan
            .object
            .as_ref()
            .and_then(|o| self.objects.get(o))
            .map(|(_, origin)| origin.clone());
        let origin = overrides
            .last()
            .map(|(_, s)| s.origin.clone())
            .or_else(|| object_origin.clone())
            .unwrap_or_else(|| host.origin.clone());
        if plan.object.is_none()
            && let Err(msg) = valid_name(name)
        {
            self.sink.error(Some(&origin), format!("{what}: {msg}"));
            return None;
        }

        // Meta keys: the most specific wins; groups add up.
        let mut meta = object.clone().unwrap_or_default();
        for (t, src) in &overrides {
            meta.add(t, src);
        }
        let meta_def = self.settings(&meta.table, &origin, &what)?;
        if meta_def.enabled == Some(false) {
            return None;
        }
        let check_name = match meta_def.check {
            Some(c) => Some(c),
            None if self.checks.contains_key(&plan.base) => Some(plan.base.clone()),
            None => None,
        };

        let mut layered = Layered::default();
        let mut layers = Vec::new();
        let add_paths = |layered: &mut Layered, from: &Layered| {
            for (path, src) in &from.sources {
                let key = path.split('.').next().unwrap_or_default();
                if META_KEYS.contains(&key) {
                    continue;
                }
                if let Some(v) = get_path(&from.table, path) {
                    let mut t = Table::new();
                    set_path(&mut t, path, v.clone());
                    layered.add(&t, src);
                }
            }
        };
        if let Some(check) = &check_name {
            let Some((table, check_origin)) = self.checks.get(check) else {
                self.sink
                    .error(Some(&origin), format!("{what}: unknown check {check:?}"));
                return None;
            };
            let src = Source {
                layer: Layer::Check,
                name: check.clone(),
                origin: check_origin.clone(),
            };
            let check_layer = Layered::default().with(table, &src);
            add_paths(&mut layered, &check_layer);
            layers.push(src);
        }
        if let (Some(obj), Some(name), Some(o)) = (&object, &plan.object, &object_origin) {
            add_paths(&mut layered, obj);
            layers.push(Source {
                layer: Layer::Service,
                name: name.clone(),
                origin: o.clone(),
            });
        }
        // Host vars (text, including those of its host groups) override the
        // service and check var defaults.
        for (key, var) in &host.vars {
            if let Var::Text(text) = var {
                let src = host
                    .sources
                    .get(&format!("vars.{key}"))
                    .unwrap_or(&host_src);
                let mut t = Table::new();
                set_path(&mut t, &format!("vars.{key}"), Value::String(text.clone()));
                layered.add(&t, src);
            }
        }
        for g in &host.groups {
            if let Some(info) = self.hostgroups.get(&g.name)
                && !info.def.vars.is_empty()
            {
                layers.push(Source {
                    layer: Layer::HostGroup,
                    name: g.name.clone(),
                    origin: info.origin.clone(),
                });
            }
        }
        layers.push(host_src.clone());
        let for_each = plan.for_each.as_ref().map(|(var, key, vars)| {
            let src = Source {
                layer: Layer::ForEach,
                name: format!("{}.vars.{var}.{key}", host.name),
                origin: host
                    .sources
                    .get(&format!("vars.{var}.{key}"))
                    .or_else(|| host.sources.get(&format!("vars.{var}")))
                    .map_or_else(|| host.origin.clone(), |s| s.origin.clone()),
            };
            let mut t = Table::new();
            t.insert("vars".into(), Value::Table(vars.clone()));
            layered.add(&t, &src);
            layers.push(src);
            (var.clone(), key.clone())
        });
        for (table, src) in &overrides {
            let o = Layered::default().with(table, src);
            add_paths(&mut layered, &o);
            layers.push(src.clone());
        }

        let def = self.settings(&layered.table, &origin, &what)?;
        let settings = self.resolve_settings(&def, &origin, &what)?;
        let vars: BTreeMap<String, String> = def
            .vars
            .iter()
            .filter_map(|(k, v)| Some((k.clone(), v.as_text()?.to_string())))
            .collect();

        let command = match (&def.command, settings.passive) {
            (None, false) => {
                let msg = match &check_name {
                    Some(c) => {
                        format!("{what}: check {c:?} has no command (or set passive = true)")
                    }
                    None => format!(
                        "{what}: no service or check {:?}; set check, command or passive = true",
                        plan.base
                    ),
                };
                self.sink.error(Some(&origin), msg);
                return None;
            }
            (Some(_), true) => {
                self.sink.error(
                    Some(&origin),
                    format!("{what}: passive services have no command"),
                );
                return None;
            }
            (Some(text), false) => {
                let lookup = |m: &str| -> Result<String, String> {
                    let value = match macros::parse(m) {
                        Some(Ok(macros::Fixed::HostName)) => Some(host.name.clone()),
                        Some(Ok(macros::Fixed::HostAddress)) => Some(host.address.clone()),
                        Some(Ok(macros::Fixed::HostNode)) => host.node.clone(),
                        Some(Ok(macros::Fixed::HostVar(v))) => var_at(&host.vars, v)
                            .and_then(Var::as_text)
                            .map(String::from),
                        Some(Ok(macros::Fixed::ServiceName)) => Some(name.to_string()),
                        Some(Ok(macros::Fixed::ServiceKey)) => {
                            plan.for_each.as_ref().map(|(_, key, _)| key.clone())
                        }
                        Some(Ok(macros::Fixed::CheckName)) => check_name.clone(),
                        Some(Err(var)) => vars.get(var).cloned(),
                        None => return Err(format!("unknown macro ${m}$")),
                    };
                    value.ok_or_else(|| format!("${m}$ is not set"))
                };
                match macros::expand(text, settings.shell, lookup) {
                    Ok(cmd) => Some(cmd),
                    Err(msg) => {
                        self.sink.error(Some(&origin), format!("{what}: {msg}"));
                        return None;
                    }
                }
            }
            (None, true) => None,
        };

        // Service groups: those listing its service object or listed by it,
        // and those its host service lists.
        let mut groups: Vec<Assigned> = Vec::new();
        if let Some(object) = &plan.object {
            for (sg, members) in &self.sg_services {
                if let Some((_, reason)) = members.iter().find(|(s, _)| s == object) {
                    groups.push(Assigned {
                        name: sg.clone(),
                        reason: reason.clone(),
                    });
                }
            }
        }
        for (table, src) in &overrides {
            let Some(Value::Array(items)) = table.get("groups") else {
                continue;
            };
            for g in items.iter().filter_map(Value::as_str) {
                if groups.iter().any(|a| a.name == g) {
                    continue;
                }
                if !self.sg_services.contains_key(g) {
                    self.sink.error(
                        Some(&src.origin),
                        format!("{what}: unknown servicegroup {g:?}"),
                    );
                    continue;
                }
                let (h, svc) = src.name.split_once('.').unwrap_or((&src.name, ""));
                groups.push(Assigned {
                    name: g.to_string(),
                    reason: Reason::Listed {
                        by: format!("host {h}, service {svc}"),
                        origin: src.origin.clone(),
                    },
                });
            }
        }

        Some(Service {
            id: format!("{}/{name}", host.name),
            name: name.to_string(),
            host: host_idx,
            check: check_name,
            object: plan.object.clone(),
            assigned: plan.assigned.clone(),
            for_each,
            check_hash: check_hash(command.as_ref(), settings.format),
            command_text: def.command.clone(),
            command,
            settings,
            vars,
            groups,
            origin,
            sources: layered.sources,
            layers,
        })
    }

    fn compile_downtimegroups(&mut self, config: &mut Config) {
        for raw in &self.raw.downtimegroups {
            if self.duplicate(
                "downtimegroup",
                &raw.name,
                &raw.origin,
                &self.raw.downtimegroups,
            ) {
                continue;
            }
            let what = format!("downtimegroup {:?}", raw.name);
            if let Err(msg) = valid_name(&raw.name) {
                self.sink.error(Some(&raw.origin), format!("{what}: {msg}"));
                continue;
            }
            let def: DowntimeGroupDef =
                match DowntimeGroupDef::deserialize(Value::Table(raw.table.clone())) {
                    Ok(d) => d,
                    Err(err) => {
                        self.sink.error(
                            Some(&raw.origin),
                            format!("{what}: {}", err.message().trim()),
                        );
                        continue;
                    }
                };
            let before = self.sink.diagnostics.len();
            for g in &def.hostgroups {
                if !config.hostgroups.iter().any(|x| x.name == *g) {
                    self.sink.error(
                        Some(&raw.origin),
                        format!("{what}: unknown hostgroup {g:?}"),
                    );
                }
            }
            let mut schedule = Vec::new();
            for (i, entry) in def.schedule.iter().enumerate() {
                match crate::Window::parse(entry) {
                    Ok(w) => schedule.push(w),
                    Err(e) => self.sink.error(
                        Some(&raw.origin),
                        format!("{what}: schedule {}: {e}", i + 1),
                    ),
                }
            }
            if self.sink.diagnostics.len() > before {
                continue;
            }
            config.downtimegroups.push(crate::DowntimeGroup {
                name: raw.name.clone(),
                title: def.title,
                members: def.members,
                hostgroups: def.hostgroups,
                schedule,
                origin: raw.origin.clone(),
            });
        }
    }

    /// Warnings for objects used by nothing and selector names that exist
    /// nowhere (likely typos).
    fn check_references(&mut self, config: &Config) {
        for check in &config.checks {
            let used = config
                .service_objects
                .iter()
                .any(|s| s.check.as_ref() == Some(&check.name))
                || config
                    .services
                    .iter()
                    .any(|s| s.check.as_ref() == Some(&check.name));
            if !used {
                self.sink.warning(
                    Some(&check.origin),
                    format!("check {:?} is used by no service", check.name),
                );
            }
        }
        for object in &config.service_objects {
            if !config
                .services
                .iter()
                .any(|s| s.object.as_ref() == Some(&object.name))
            {
                self.sink.warning(
                    Some(&object.origin),
                    format!("service {:?} is on no host", object.name),
                );
            }
        }
        for g in &config.hostgroups {
            if !config.hosts.iter().any(|h| h.in_group(&g.name)) {
                self.sink.warning(
                    Some(&g.origin),
                    format!("hostgroup {:?} has no hosts", g.name),
                );
            }
        }
        for g in &config.downtimegroups {
            if !config.hosts.iter().any(|h| g.has_host(h)) {
                self.sink.warning(
                    Some(&g.origin),
                    format!("downtimegroup {:?} has no hosts", g.name),
                );
            }
        }
        for g in &config.servicegroups {
            if self.sg_services.get(&g.name).is_none_or(Vec::is_empty) {
                self.sink.warning(
                    Some(&g.origin),
                    format!("servicegroup {:?} has no services", g.name),
                );
            }
        }
        let mut problems: Vec<(Origin, String)> = Vec::new();
        let mut check = |origin: &Origin, kind: Ref, name: &str| {
            let (what, known) = match kind {
                Ref::Host => ("host", config.hosts.iter().any(|h| h.name == name)),
                Ref::Node => (
                    "node",
                    config.hosts.iter().any(|h| h.node.as_deref() == Some(name)),
                ),
                Ref::HostGroup => (
                    "hostgroup",
                    config.hostgroups.iter().any(|g| g.name == name),
                ),
                Ref::Check => ("check", config.checks.iter().any(|c| c.name == name)),
                Ref::ServiceGroup => (
                    "servicegroup",
                    config.servicegroups.iter().any(|g| g.name == name),
                ),
            };
            if !known {
                problems.push((origin.clone(), format!("selector: no {what} {name:?}")));
            }
        };
        for rule in &config.notify {
            if let Some(sel) = &rule.hosts {
                sel.refs(&mut |k, n| check(&rule.origin, k, n));
            }
            if let Some(sel) = &rule.services {
                sel.refs(&mut |k, n| check(&rule.origin, k, n));
            }
        }
        for (origin, msg) in problems {
            self.sink.warning(Some(&origin), msg);
        }
    }

    /// Settings with defaults applied, validated.
    fn resolve_settings(
        &mut self,
        def: &SettingsDef,
        origin: &Origin,
        what: &str,
    ) -> Option<Settings> {
        let before = self.sink.diagnostics.len();
        let mut error = |msg: String| self.sink.error(Some(origin), format!("{what}: {msg}"));
        let passive = def.passive.unwrap_or(false);
        if def.freshness.is_some() && !passive {
            error("freshness only applies to passive services".into());
        }
        if def.freshness.is_some_and(|f| f.is_zero()) {
            error("freshness must be greater than 0".into());
        }
        let interval = def.interval.unwrap_or(Settings::DEFAULT_INTERVAL);
        let retry_interval = def
            .retry_interval
            .unwrap_or(interval.min(Settings::DEFAULT_INTERVAL));
        for (key, value) in [("interval", interval), ("retry_interval", retry_interval)] {
            if value.is_zero() {
                error(format!("{key} must be greater than 0"));
            }
        }
        let timeout = def.timeout.unwrap_or(Settings::DEFAULT_TIMEOUT);
        if timeout.is_zero() {
            error("timeout must be greater than 0".into());
        }
        // A passive result may come once a day: confirming a problem over
        // several results would delay the alert by days.
        let max_attempts = def.max_attempts.unwrap_or(match passive {
            true => 1,
            false => Settings::DEFAULT_MAX_ATTEMPTS,
        });
        if max_attempts == 0 {
            error("max_attempts must be at least 1".into());
        }
        let flap = FlapThresholds {
            low: def.flap_low.unwrap_or(FlapThresholds::DEFAULT.low),
            high: def.flap_high.unwrap_or(FlapThresholds::DEFAULT.high),
        };
        if !(0.0..=100.0).contains(&flap.low) || !(0.0..=100.0).contains(&flap.high) {
            error("flap_low and flap_high are percentages (0 to 100)".into());
        } else if flap.low > flap.high {
            error("flap_low must not be above flap_high".into());
        }
        if self.sink.diagnostics.len() > before {
            return None;
        }
        Some(Settings {
            shell: def.shell.unwrap_or(false),
            passive,
            format: def.format.unwrap_or_default(),
            interval,
            retry_interval,
            max_attempts,
            timeout,
            freshness: def.freshness,
            flap: def.flap_detection.unwrap_or(true).then_some(flap),
            thresholds: def.thresholds.clone(),
            graphs: def.graphs.unwrap_or(true),
        })
    }

    fn compile_contacts(&mut self, config: &mut Config) {
        for raw in &self.raw.contacts {
            if self.duplicate("contact", &raw.name, &raw.origin, &self.raw.contacts) {
                continue;
            }
            let what = format!("contact {:?}", raw.name);
            let def: ContactDef = match ContactDef::deserialize(Value::Table(raw.table.clone())) {
                Ok(d) => d,
                Err(err) => {
                    self.sink.error(
                        Some(&raw.origin),
                        format!("{what}: {}", err.message().trim()),
                    );
                    continue;
                }
            };
            let mut ok = true;
            if let Err(msg) = valid_name(&raw.name) {
                self.sink.error(Some(&raw.origin), format!("{what}: {msg}"));
                ok = false;
            }
            for email in &def.email {
                if !email.contains('@') {
                    self.sink.error(
                        Some(&raw.origin),
                        format!("{what}: invalid email {email:?}"),
                    );
                    ok = false;
                }
            }
            if let Some(url) = &def.webhook
                && !(url.starts_with("http://") || url.starts_with("https://"))
            {
                self.sink.error(
                    Some(&raw.origin),
                    format!("{what}: webhook must be an http(s) URL"),
                );
                ok = false;
            }
            if def.email.is_empty() && def.webhook.is_none() {
                self.sink
                    .warning(Some(&raw.origin), format!("{what} has no email or webhook"));
            }
            if !ok {
                continue;
            }
            config.contacts.push(Contact {
                name: raw.name.clone(),
                email: def.email,
                webhook: def.webhook,
                groups: def
                    .groups
                    .into_iter()
                    .map(|g| Assigned {
                        name: g,
                        reason: Reason::Listed {
                            by: format!("contact {}", raw.name),
                            origin: raw.origin.clone(),
                        },
                    })
                    .collect(),
                origin: raw.origin.clone(),
            });
        }
        for raw in &self.raw.contactgroups {
            if self.duplicate(
                "contactgroup",
                &raw.name,
                &raw.origin,
                &self.raw.contactgroups,
            ) {
                continue;
            }
            let what = format!("contactgroup {:?}", raw.name);
            if let Err(msg) = valid_name(&raw.name) {
                self.sink.error(Some(&raw.origin), format!("{what}: {msg}"));
                continue;
            }
            let def: ContactGroupDef =
                match ContactGroupDef::deserialize(Value::Table(raw.table.clone())) {
                    Ok(d) => d,
                    Err(err) => {
                        self.sink.error(
                            Some(&raw.origin),
                            format!("{what}: {}", err.message().trim()),
                        );
                        continue;
                    }
                };
            for member in &def.members {
                match config.contacts.iter_mut().find(|c| c.name == *member) {
                    None => self.sink.error(
                        Some(&raw.origin),
                        format!("{what}: unknown contact {member:?}"),
                    ),
                    Some(c) if c.groups.iter().any(|g| g.name == raw.name) => {}
                    Some(c) => c.groups.push(Assigned {
                        name: raw.name.clone(),
                        reason: Reason::Listed {
                            by: format!("contactgroup {}", raw.name),
                            origin: raw.origin.clone(),
                        },
                    }),
                }
            }
            config.contactgroups.push(Group {
                name: raw.name.clone(),
                title: def.title,
                members: def.members,
                services: Vec::new(),
                servicegroups: Vec::new(),
                origin: raw.origin.clone(),
            });
        }
        for c in &config.contacts {
            for g in &c.groups {
                if !config.contactgroups.iter().any(|x| x.name == g.name) {
                    self.sink.error(
                        Some(&c.origin),
                        format!("contact {:?}: unknown contactgroup {:?}", c.name, g.name),
                    );
                }
            }
        }
    }

    fn compile_notify(&mut self, config: &mut Config) {
        for raw in &self.raw.notify {
            let what = match raw.table.get("name").and_then(Value::as_str) {
                Some(n) => format!("notify {n:?}"),
                None => "notify".to_string(),
            };
            let def: NotifyDef = match NotifyDef::deserialize(Value::Table(raw.table.clone())) {
                Ok(d) => d,
                Err(err) => {
                    self.sink.error(
                        Some(&raw.origin),
                        format!("{what}: {}", err.message().trim()),
                    );
                    continue;
                }
            };
            let before = self.sink.diagnostics.len();
            let mut error =
                |msg: String| self.sink.error(Some(&raw.origin), format!("{what}: {msg}"));
            if let Some(name) = &def.name {
                if let Err(msg) = valid_name(name) {
                    error(msg.into());
                }
                if config.notify.iter().any(|r| r.name.as_ref() == Some(name)) {
                    error("name is already used by another rule".into());
                }
            }
            if def.contacts.is_empty() && def.contact_groups.is_empty() {
                error("needs contacts or contact_groups".into());
            }
            for c in &def.contacts {
                if !config.contacts.iter().any(|x| x.name == *c) {
                    error(format!("unknown contact {c:?}"));
                }
            }
            for g in &def.contact_groups {
                if !config.contactgroups.iter().any(|x| x.name == *g) {
                    error(format!("unknown contactgroup {g:?}"));
                }
            }
            for s in &def.states {
                if !["warning", "critical", "unknown", "down"].contains(&s.as_str()) {
                    error(format!(
                        "unknown state {s:?} (expected warning, critical, unknown or down)"
                    ));
                }
            }
            let mut on = NotifyOn {
                problem: true,
                recovery: true,
                flapping: true,
                downtime: true,
            };
            if let Some(list) = &def.on {
                on = NotifyOn {
                    problem: false,
                    recovery: false,
                    flapping: false,
                    downtime: false,
                };
                for item in list {
                    match item.as_str() {
                        "problem" => on.problem = true,
                        "recovery" => on.recovery = true,
                        "flapping" => on.flapping = true,
                        "downtime" => on.downtime = true,
                        other => error(format!(
                            "on: unknown {other:?} (expected problem, recovery, flapping or downtime)"
                        )),
                    }
                }
            }
            let hosts = def.hosts.as_ref().and_then(|v| {
                HostSelector::parse(v)
                    .map_err(|e| error(format!("hosts: {e}")))
                    .ok()
            });
            let services = def.services.as_ref().and_then(|v| {
                ServiceSelector::parse(v)
                    .map_err(|e| error(format!("services: {e}")))
                    .ok()
            });
            if self.sink.diagnostics.len() > before {
                continue;
            }
            config.notify.push(NotifyRule {
                name: def.name,
                contacts: def.contacts,
                contact_groups: def.contact_groups,
                hosts,
                services,
                states: def.states,
                on,
                enabled: def.enabled,
                origin: raw.origin.clone(),
            });
        }
    }

    fn compile_sinks(&mut self, config: &mut Config) {
        for raw in &self.raw.sinks {
            let Some(s) = self.sink_object(raw) else {
                continue;
            };
            if let Some(prev) = config.sinks.iter().find(|p| p.name == s.name) {
                let msg = format!("sink {:?} is already defined at {}", s.name, prev.origin);
                self.sink.error(Some(&raw.origin), msg);
                continue;
            }
            config.sinks.push(s);
        }
    }

    /// Whether `name` was already defined by an earlier object of `list`;
    /// reports it if so.
    fn duplicate(&mut self, kind: &str, name: &str, origin: &Origin, list: &[RawObject]) -> bool {
        let first = list.iter().find(|o| o.name == name);
        match first {
            Some(first) if first.origin != *origin => {
                self.sink.error(
                    Some(origin),
                    format!("{kind} {name:?} is already defined at {}", first.origin),
                );
                true
            }
            _ => false,
        }
    }

    /// Validates a check, service or override table.
    fn settings(&mut self, table: &Table, origin: &Origin, what: &str) -> Option<SettingsDef> {
        match SettingsDef::deserialize(Value::Table(table.clone())) {
            Ok(def) => Some(def),
            Err(err) => {
                self.sink
                    .error(Some(origin), format!("{what}: {}", err.message().trim()));
                None
            }
        }
    }

    fn check_command(&mut self, command: &str, shell: bool, origin: &Origin, what: &str) {
        if command.trim().is_empty() {
            self.sink
                .error(Some(origin), format!("{what}: command is empty"));
        } else if !shell && let Err(err) = shell_words::split(command) {
            self.sink
                .error(Some(origin), format!("{what}: cannot split command: {err}"));
        }
        for name in macros::names(command) {
            if macros::parse(name).is_none() {
                self.sink
                    .error(Some(origin), format!("{what}: unknown macro ${name}$"));
            }
        }
    }

    fn sink_object(&mut self, raw: &RawObject) -> Option<crate::Sink> {
        let def: SinkDef = match SinkDef::deserialize(Value::Table(raw.table.clone())) {
            Ok(def) => def,
            Err(err) => {
                self.sink
                    .error(Some(&raw.origin), err.message().trim().to_string());
                return None;
            }
        };
        let origin = &raw.origin;
        let what = format!("sink {:?}", def.name);
        let errors_before = self.sink.diagnostics.len();
        let mut error = |msg: String| self.sink.error(Some(origin), format!("{what}: {msg}"));

        if let Err(msg) = valid_name(&def.name) {
            error(msg.into());
        }
        let kind = match def.kind.as_str() {
            "postgres" => match def.url {
                Some(url) if url.starts_with("postgres://") || url.starts_with("postgresql://") => {
                    // Relative to the file the sink is defined in.
                    let password_file = def
                        .password_file
                        .map(|p| origin.file.parent().map_or(p.clone(), |dir| dir.join(&p)));
                    Some(crate::SinkKind::Postgres { url, password_file })
                }
                Some(_) => {
                    error("url must start with postgres://".into());
                    None
                }
                None => {
                    error("needs url".into());
                    None
                }
            },
            "hub" => {
                error("type hub: not available yet".into());
                None
            }
            other => {
                error(format!("unknown type {other:?} (expected postgres)"));
                None
            }
        };
        let interval = def.flush.interval.unwrap_or(crate::Sink::DEFAULT_INTERVAL);
        if interval.is_zero() {
            error("flush.interval must be greater than 0".into());
        }
        let flush = match def.flush.mode.as_deref().unwrap_or("interval") {
            "interval" => crate::Flush::Interval(interval),
            "realtime" => crate::Flush::Realtime,
            mode @ ("adaptive" | "schedule") => {
                error(format!("flush.mode {mode}: not available yet"));
                crate::Flush::Interval(interval)
            }
            other => {
                error(format!(
                    "unknown flush.mode {other:?} (expected interval or realtime)"
                ));
                crate::Flush::Interval(interval)
            }
        };
        let max_buffer = match def.buffer.max_size.as_deref() {
            None => crate::Sink::DEFAULT_MAX_BUFFER,
            Some(text) => parse_size(text).unwrap_or_else(|| {
                error(format!("invalid buffer.max_size {text:?}"));
                0
            }),
        };
        match def.buffer.on_full.as_deref() {
            None | Some("drop_oldest") => {}
            Some(mode @ ("downsample" | "block")) => {
                error(format!("buffer.on_full {mode}: not available yet"))
            }
            Some(other) => error(format!(
                "unknown buffer.on_full {other:?} (expected drop_oldest)"
            )),
        }

        if self.sink.diagnostics.len() > errors_before {
            return None;
        }
        Some(crate::Sink {
            name: def.name,
            kind: kind?,
            flush,
            max_buffer,
            origin: origin.clone(),
        })
    }
}

/// FNV-1a over the command and format: stable across builds and Rust
/// versions, unlike `DefaultHasher`.
fn check_hash(command: Option<&CommandLine>, format: Format) -> String {
    let text = match command {
        Some(CommandLine::Argv(argv)) => format!("argv\0{}\0{format}", argv.join("\0")),
        Some(CommandLine::Shell(script)) => format!("shell\0{script}\0{format}"),
        None => format!("passive\0{format}"),
    };
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Names end up in service ids (`host/service`), URLs and file names.
fn valid_name(name: &str) -> Result<(), &'static str> {
    if name.trim().is_empty() {
        Err("name is empty")
    } else if name.trim() != name {
        Err("name has leading or trailing spaces")
    } else if name.contains('/') {
        Err("name must not contain '/'")
    } else if name.chars().any(char::is_control) {
        Err("name must not contain control characters")
    } else {
        Ok(())
    }
}

/// Vars from a TOML table: scalars as text, tables kept, lists refused.
fn vars_of(table: &Table) -> Result<Vars, String> {
    fn var(v: &Value, key: &str) -> Result<Var, String> {
        Ok(match v {
            Value::String(s) => Var::Text(s.clone()),
            Value::Integer(n) => Var::Text(n.to_string()),
            Value::Float(n) => Var::Text(n.to_string()),
            Value::Boolean(b) => Var::Text(b.to_string()),
            Value::Table(t) => Var::Table(
                t.iter()
                    .map(|(k, v)| Ok((k.clone(), var(v, &format!("{key}.{k}"))?)))
                    .collect::<Result<_, String>>()?,
            ),
            _ => return Err(format!("vars.{key}: expected a value or a table")),
        })
    }
    table
        .iter()
        .map(|(k, v)| Ok((k.clone(), var(v, k)?)))
        .collect()
}

/// Settings of a check, a service or a host service. Every field is
/// optional: layers are merged before the result is read.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsDef {
    title: Option<String>,
    check: Option<String>,
    command: Option<String>,
    shell: Option<bool>,
    passive: Option<bool>,
    format: Option<Format>,
    #[serde(default, deserialize_with = "vars")]
    vars: Vars,
    #[serde(default)]
    thresholds: BTreeMap<String, ThresholdSpec>,
    #[serde(default, deserialize_with = "duration")]
    interval: Option<Duration>,
    #[serde(default, deserialize_with = "duration")]
    retry_interval: Option<Duration>,
    #[serde(default, deserialize_with = "duration")]
    timeout: Option<Duration>,
    #[serde(default, deserialize_with = "duration")]
    freshness: Option<Duration>,
    max_attempts: Option<u32>,
    flap_detection: Option<bool>,
    flap_low: Option<f64>,
    flap_high: Option<f64>,
    graphs: Option<bool>,
    for_each: Option<String>,
    #[serde(default)]
    groups: Vec<String>,
    enabled: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostDef {
    name: String,
    address: Option<String>,
    node: Option<String>,
    /// Host groups.
    #[serde(default)]
    groups: Vec<String>,
    #[serde(default)]
    services: Vec<String>,
    #[serde(default)]
    servicegroups: Vec<String>,
    #[serde(default)]
    vars: Table,
    host_check: Option<String>,
    graph: Option<String>,
    #[serde(default = "yes")]
    enabled: bool,
    #[serde(default)]
    service: BTreeMap<String, Table>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostGroupDef {
    title: Option<String>,
    /// Host names and patterns.
    #[serde(default)]
    members: Vec<String>,
    #[serde(default)]
    services: Vec<String>,
    #[serde(default)]
    servicegroups: Vec<String>,
    host_check: Option<String>,
    graph: Option<String>,
    #[serde(default)]
    vars: Table,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ServiceGroupDef {
    title: Option<String>,
    #[serde(default)]
    services: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DowntimeGroupDef {
    title: Option<String>,
    #[serde(default)]
    members: Vec<String>,
    #[serde(default)]
    hostgroups: Vec<String>,
    #[serde(default)]
    schedule: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContactGroupDef {
    title: Option<String>,
    #[serde(default)]
    members: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContactDef {
    #[serde(default, deserialize_with = "one_or_many")]
    email: Vec<String>,
    webhook: Option<String>,
    #[serde(default)]
    groups: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NotifyDef {
    name: Option<String>,
    #[serde(default)]
    contacts: Vec<String>,
    #[serde(default)]
    contact_groups: Vec<String>,
    hosts: Option<Value>,
    services: Option<Value>,
    #[serde(default)]
    states: Vec<String>,
    on: Option<Vec<String>>,
    #[serde(default = "yes")]
    enabled: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SinkDef {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    url: Option<String>,
    password_file: Option<PathBuf>,
    #[serde(default)]
    flush: FlushDef,
    #[serde(default)]
    buffer: BufferDef,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FlushDef {
    mode: Option<String>,
    #[serde(default, deserialize_with = "duration")]
    interval: Option<Duration>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct BufferDef {
    max_size: Option<String>,
    on_full: Option<String>,
}

/// A size like `512MB` or `2GB` (×1024), or plain bytes.
fn parse_size(text: &str) -> Option<u64> {
    let text = text.trim();
    let split = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let number: f64 = number.parse().ok()?;
    let factor = match watcher_core::Unit::parse(unit.trim()) {
        (watcher_core::Unit::None | watcher_core::Unit::Bytes, f) => f,
        _ => return None,
    };
    let bytes = number * factor;
    (bytes.is_finite() && bytes >= 1.0).then_some(bytes as u64)
}

fn yes() -> bool {
    true
}

/// A duration like `"1m"` or `"15s"`.
fn duration<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Duration>, D::Error> {
    let Some(text) = Option::<String>::deserialize(d)? else {
        return Ok(None);
    };
    humantime::parse_duration(&text)
        .map(Some)
        .map_err(|e| D::Error::custom(format!("invalid duration {text:?}: {e}")))
}

/// Vars: scalars as text, tables (for `for_each`) kept.
fn vars<'de, D: Deserializer<'de>>(d: D) -> Result<Vars, D::Error> {
    let table = Table::deserialize(d)?;
    vars_of(&table).map_err(D::Error::custom)
}

fn one_or_many<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    Ok(match OneOrMany::deserialize(d)? {
        OneOrMany::One(s) => vec![s],
        OneOrMany::Many(v) => v,
    })
}

#[cfg(test)]
#[path = "load_tests.rs"]
mod tests;
