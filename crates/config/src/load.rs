use crate::{
    Check, Config, Contact, Diagnostic, Expr, Host, Loaded, Origin, Service, Severity, macros,
};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer};
use std::collections::{BTreeMap, HashMap, HashSet};
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

    /// Sorted by file and line; a template problem hit by several objects is
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
    pub template: BTreeMap<String, Spanned<Table>>,
    #[serde(default)]
    pub host: Vec<Spanned<Table>>,
    #[serde(default)]
    pub check: Vec<Spanned<Table>>,
    #[serde(default)]
    pub contact: BTreeMap<String, Spanned<Table>>,
    #[serde(default)]
    pub sink: Vec<Spanned<Table>>,
}

struct RawObject {
    table: Table,
    origin: Origin,
}

#[derive(Default)]
struct RawObjects {
    templates: HashMap<String, RawObject>,
    hosts: Vec<RawObject>,
    checks: Vec<RawObject>,
    contacts: Vec<(String, RawObject)>,
    sinks: Vec<RawObject>,
}

impl RawObjects {
    fn add_file(&mut self, path: &Path, text: &str, sink: &mut Sink) {
        let origin = |offset: usize| Origin {
            file: path.to_path_buf(),
            line: text[..offset.min(text.len())].matches('\n').count() + 1,
        };
        let file: RawFile = match toml::from_str(text) {
            Ok(file) => file,
            Err(err) => {
                let at = err.span().map_or(
                    Origin {
                        file: path.into(),
                        line: 0,
                    },
                    |s| origin(s.start),
                );
                sink.error(Some(&at), err.message().to_string());
                return;
            }
        };
        let object = |spanned: Spanned<Table>| RawObject {
            origin: origin(spanned.span().start),
            table: spanned.into_inner(),
        };

        for (name, t) in file.template {
            let t = object(t);
            if let Some(prev) = self.templates.get(&name) {
                sink.error(
                    Some(&t.origin),
                    format!("template {name:?} is already defined at {}", prev.origin),
                );
                continue;
            }
            self.templates.insert(name, t);
        }
        self.hosts.extend(file.host.into_iter().map(object));
        self.checks.extend(file.check.into_iter().map(object));
        self.contacts
            .extend(file.contact.into_iter().map(|(name, t)| (name, object(t))));
        self.sinks.extend(file.sink.into_iter().map(object));
    }

    fn compile(&self, sink: &mut Sink) -> Config {
        let mut config = Config::default();

        for (name, raw) in &self.contacts {
            if let Some(contact) = self.contact(name, raw, sink) {
                if let Some(prev) = config.contacts.iter().find(|c| c.name == *name) {
                    let msg = format!("contact {name:?} is already defined at {}", prev.origin);
                    sink.error(Some(&raw.origin), msg);
                } else {
                    config.contacts.push(contact);
                }
            }
        }

        let mut host_names = HashMap::new();
        for raw in &self.hosts {
            let Some(host) = self.host(raw, sink) else {
                continue;
            };
            if let Some(prev) = host_names.get(&host.name) {
                let msg = format!("host {:?} is already defined at {prev}", host.name);
                sink.error(Some(&raw.origin), msg);
                continue;
            }
            host_names.insert(host.name.clone(), raw.origin.clone());
            config.hosts.push(host);
        }

        let contacts: HashSet<_> = config.contacts.iter().map(|c| c.name.as_str()).collect();
        let mut check_names = HashMap::new();
        for raw in &self.checks {
            let Some(check) = self.check(raw, &host_names, &contacts, sink) else {
                continue;
            };
            if let Some(prev) = check_names.get(&check.name) {
                let msg = format!("check {:?} is already defined at {prev}", check.name);
                sink.error(Some(&raw.origin), msg);
                continue;
            }
            check_names.insert(check.name.clone(), raw.origin.clone());
            config.checks.push(check);
        }

        config.services = services(&config, sink);
        for host in &config.hosts {
            if let Some(check) = &host.check
                && host.enabled
                && config.host_check(host).is_none()
            {
                let msg = if config.checks.iter().any(|c| c.name == *check) {
                    format!("host {:?}: check {check:?} does not apply to it", host.name)
                } else {
                    format!("host {:?}: unknown check {check:?}", host.name)
                };
                sink.error(Some(&host.origin), msg);
            }
            // Only for display: a warning.
            if let Some(graph) = &host.graph
                && host.enabled
                && !config
                    .services
                    .iter()
                    .any(|s| config.host(s).name == host.name && config.check(s).name == *graph)
            {
                let msg = format!("host {:?}: graph: no service {graph:?} on it", host.name);
                sink.warning(Some(&host.origin), msg);
            }
        }

        for raw in &self.sinks {
            let Some(s) = self.sink(raw, sink) else {
                continue;
            };
            if let Some(prev) = config.sinks.iter().find(|p| p.name == s.name) {
                let msg = format!("sink {:?} is already defined at {}", s.name, prev.origin);
                sink.error(Some(&raw.origin), msg);
                continue;
            }
            config.sinks.push(s);
        }
        config
    }

    /// Merges the templates named in `use` (recursively) under `table`.
    fn resolve(
        &self,
        table: &Table,
        origin: &Origin,
        stack: &mut Vec<String>,
        sink: &mut Sink,
    ) -> Option<Table> {
        let mut table = table.clone();
        let uses = match table.remove("use") {
            None => Vec::new(),
            Some(Value::String(name)) => vec![name],
            Some(Value::Array(items)) => {
                let names: Option<Vec<String>> = items
                    .into_iter()
                    .map(|v| v.as_str().map(String::from))
                    .collect();
                let Some(names) = names else {
                    sink.error(Some(origin), "use: expected template names");
                    return None;
                };
                names
            }
            Some(_) => {
                sink.error(
                    Some(origin),
                    "use: expected a template name or a list of names",
                );
                return None;
            }
        };

        let mut merged = Table::new();
        for name in uses {
            if stack.contains(&name) {
                let cycle = [stack.as_slice(), &[name]].concat().join(" -> ");
                sink.error(Some(origin), format!("template cycle: {cycle}"));
                return None;
            }
            let Some(template) = self.templates.get(&name) else {
                sink.error(Some(origin), format!("unknown template {name:?}"));
                return None;
            };
            stack.push(name);
            let base = self.resolve(&template.table, &template.origin, stack, sink)?;
            stack.pop();
            deep_merge(&mut merged, base);
        }
        deep_merge(&mut merged, table);
        Some(merged)
    }

    /// Resolves templates and deserializes the result into `T`.
    fn typed<T: for<'de> Deserialize<'de>>(&self, raw: &RawObject, sink: &mut Sink) -> Option<T> {
        let table = self.resolve(&raw.table, &raw.origin, &mut Vec::new(), sink)?;
        match T::deserialize(Value::Table(table)) {
            Ok(t) => Some(t),
            Err(err) => {
                sink.error(Some(&raw.origin), err.message().trim().to_string());
                None
            }
        }
    }

    fn host(&self, raw: &RawObject, sink: &mut Sink) -> Option<Host> {
        let def: HostDef = self.typed(raw, sink)?;
        let what = format!("host {:?}", def.name);
        if let Err(msg) = valid_name(&def.name) {
            sink.error(Some(&raw.origin), format!("{what}: {msg}"));
            return None;
        }
        Some(Host {
            address: def.address.unwrap_or_else(|| def.name.clone()),
            check: def.check,
            graph: def.graph,
            name: def.name,
            node: def.node,
            tags: def.tags,
            vars: def.vars,
            enabled: def.enabled,
            origin: raw.origin.clone(),
        })
    }

    fn check(
        &self,
        raw: &RawObject,
        hosts: &HashMap<String, Origin>,
        contacts: &HashSet<&str>,
        sink: &mut Sink,
    ) -> Option<Check> {
        let def: CheckDef = self.typed(raw, sink)?;
        let origin = &raw.origin;
        let what = format!("check {:?}", def.name);
        let errors_before = sink.diagnostics.len();
        let mut error = |msg: String| sink.error(Some(origin), format!("{what}: {msg}"));

        if let Err(msg) = valid_name(&def.name) {
            error(msg.into());
        }
        match (&def.command, def.passive) {
            (None, false) => error("needs a command, or passive = true".into()),
            (Some(_), true) => error("passive checks have no command".into()),
            (None, true) => {}
            (Some(command), false) => {
                if command.trim().is_empty() {
                    error("command is empty".into());
                } else if !def.shell
                    && let Err(err) = shell_words::split(command)
                {
                    error(format!("cannot split command: {err}"));
                }
                for name in macros::names(command) {
                    if !macros::is_known(name) {
                        error(format!("unknown macro ${name}$"));
                    }
                }
            }
        }
        if def.freshness.is_some() && !def.passive {
            error("freshness only applies to passive checks".into());
        }
        if def.freshness.is_some_and(|f| f.is_zero()) {
            error("freshness must be greater than 0".into());
        }

        let interval = def.interval.unwrap_or(Check::DEFAULT_INTERVAL);
        let retry_interval = def
            .retry_interval
            .unwrap_or(interval.min(Check::DEFAULT_INTERVAL));
        for (key, value) in [("interval", interval), ("retry_interval", retry_interval)] {
            if value.is_zero() {
                error(format!("{key} must be greater than 0"));
            }
        }
        let timeout = def.timeout.unwrap_or(Check::DEFAULT_TIMEOUT);
        if timeout.is_zero() {
            error("timeout must be greater than 0".into());
        }
        // A passive result may come once a day: confirming a problem over
        // several results would delay the alert by days.
        let default_attempts = match def.passive {
            true => 1,
            false => Check::DEFAULT_MAX_ATTEMPTS,
        };
        let max_attempts = def.max_attempts.unwrap_or(default_attempts);
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

        let apply_to = match def.apply_to.as_deref().map(Expr::parse).transpose() {
            Ok(expr) => expr,
            Err(err) => {
                error(format!("apply_to: {err}"));
                None
            }
        };
        if def.apply_to.is_none() && def.hosts.is_empty() {
            error("needs apply_to or hosts".into());
        }
        for host in &def.hosts {
            if !hosts.contains_key(host) {
                error(format!("unknown host {host:?}"));
            }
        }
        for contact in &def.notify {
            if !contacts.contains(contact.as_str()) {
                error(format!("unknown contact {contact:?}"));
            }
        }

        if sink.diagnostics.len() > errors_before {
            return None;
        }
        Some(Check {
            name: def.name,
            command: def.command,
            shell: def.shell,
            format: def.format,
            interval,
            retry_interval,
            max_attempts,
            timeout,
            freshness: def.freshness,
            flap: def.flap_detection.then_some(flap),
            thresholds: def.thresholds,
            notify: def.notify,
            vars: def.vars,
            apply_to,
            hosts: def.hosts,
            enabled: def.enabled,
            origin: origin.clone(),
        })
    }

    fn sink(&self, raw: &RawObject, sink: &mut Sink) -> Option<crate::Sink> {
        let def: SinkDef = self.typed(raw, sink)?;
        let origin = &raw.origin;
        let what = format!("sink {:?}", def.name);
        let errors_before = sink.diagnostics.len();
        let mut error = |msg: String| sink.error(Some(origin), format!("{what}: {msg}"));

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

        if sink.diagnostics.len() > errors_before {
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

    fn contact(&self, name: &str, raw: &RawObject, sink: &mut Sink) -> Option<Contact> {
        let def: ContactDef = self.typed(raw, sink)?;
        let origin = &raw.origin;
        let mut ok = true;
        if let Err(msg) = valid_name(name) {
            sink.error(Some(origin), format!("contact {name:?}: {msg}"));
            ok = false;
        }
        for email in &def.email {
            if !email.contains('@') {
                sink.error(
                    Some(origin),
                    format!("contact {name:?}: invalid email {email:?}"),
                );
                ok = false;
            }
        }
        if let Some(url) = &def.webhook
            && !(url.starts_with("http://") || url.starts_with("https://"))
        {
            sink.error(
                Some(origin),
                format!("contact {name:?}: webhook must be an http(s) URL"),
            );
            ok = false;
        }
        if def.email.is_empty() && def.webhook.is_none() {
            sink.warning(
                Some(origin),
                format!("contact {name:?} has no email or webhook"),
            );
        }
        ok.then(|| Contact {
            name: name.to_string(),
            email: def.email,
            webhook: def.webhook,
            origin: origin.clone(),
        })
    }
}

/// Expands every enabled check onto the enabled hosts it applies to.
fn services(config: &Config, sink: &mut Sink) -> Vec<Service> {
    let mut services = Vec::new();
    for (check_idx, check) in config.checks.iter().enumerate() {
        if !check.enabled {
            continue;
        }
        let mut matched = 0;
        for (host_idx, host) in config.hosts.iter().enumerate() {
            let applies = check.hosts.contains(&host.name)
                || check.apply_to.as_ref().is_some_and(|e| e.matches(host));
            if !applies || !host.enabled {
                continue;
            }
            matched += 1;
            let command = check
                .command
                .as_deref()
                .map(|command| macros::expand(command, check, host))
                .transpose();
            match command {
                Ok(command) => services.push(Service {
                    id: format!("{}/{}", host.name, check.name),
                    host: host_idx,
                    check: check_idx,
                    check_hash: check_hash(command.as_ref(), check.format),
                    command,
                }),
                Err(msg) => sink.error(
                    Some(&check.origin),
                    format!("check {:?}: {msg}", check.name),
                ),
            }
        }
        if matched == 0 {
            sink.warning(
                Some(&check.origin),
                format!("check {:?} applies to no enabled host", check.name),
            );
        }
    }
    services
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

/// Merges `from` into `into`; nested tables are merged, everything else is
/// replaced.
fn deep_merge(into: &mut Table, from: Table) {
    for (key, value) in from {
        match (into.get_mut(&key), value) {
            (Some(Value::Table(a)), Value::Table(b)) => deep_merge(a, b),
            (_, value) => {
                into.insert(key, value);
            }
        }
    }
}

/// Names end up in service ids (`host/check`), URLs and file names.
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostDef {
    name: String,
    address: Option<String>,
    check: Option<String>,
    graph: Option<String>,
    node: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default, deserialize_with = "vars")]
    vars: BTreeMap<String, String>,
    #[serde(default = "yes")]
    enabled: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckDef {
    name: String,
    command: Option<String>,
    #[serde(default)]
    passive: bool,
    #[serde(default)]
    shell: bool,
    #[serde(default)]
    format: Format,
    apply_to: Option<String>,
    #[serde(default)]
    hosts: Vec<String>,
    #[serde(default, deserialize_with = "duration")]
    interval: Option<Duration>,
    #[serde(default, deserialize_with = "duration")]
    retry_interval: Option<Duration>,
    #[serde(default, deserialize_with = "duration")]
    timeout: Option<Duration>,
    #[serde(default, deserialize_with = "duration")]
    freshness: Option<Duration>,
    max_attempts: Option<u32>,
    #[serde(default)]
    thresholds: BTreeMap<String, ThresholdSpec>,
    #[serde(default)]
    notify: Vec<String>,
    #[serde(default, deserialize_with = "vars")]
    vars: BTreeMap<String, String>,
    #[serde(default = "yes")]
    flap_detection: bool,
    flap_low: Option<f64>,
    flap_high: Option<f64>,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContactDef {
    #[serde(default, deserialize_with = "one_or_many")]
    email: Vec<String>,
    webhook: Option<String>,
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

/// Scalar variables; numbers and booleans are kept as text.
fn vars<'de, D: Deserializer<'de>>(d: D) -> Result<BTreeMap<String, String>, D::Error> {
    let table = Table::deserialize(d)?;
    table
        .into_iter()
        .map(|(k, v)| {
            let text = match v {
                Value::String(s) => s,
                Value::Integer(n) => n.to_string(),
                Value::Float(n) => n.to_string(),
                Value::Boolean(b) => b.to_string(),
                _ => {
                    return Err(D::Error::custom(format!(
                        "vars.{k}: expected a string, number or boolean"
                    )));
                }
            };
            Ok((k, text))
        })
        .collect()
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
mod tests {
    use super::*;

    /// Compiles in-memory files given as `(path, text)`.
    fn compile(files: &[(&str, &str)]) -> (Config, Vec<Diagnostic>) {
        let mut sink = Sink::default();
        let mut raw = RawObjects::default();
        for (path, text) in files {
            raw.add_file(Path::new(path), text, &mut sink);
        }
        let config = raw.compile(&mut sink);
        (config, sink.finish())
    }

    fn messages(diags: &[Diagnostic]) -> Vec<String> {
        diags.iter().map(|d| d.to_string()).collect()
    }

    const EXAMPLE: &str = r#"
[template.linux]
interval = "1m"
retry_interval = "15s"
max_attempts = 3
notify = ["oncall"]

[contact.oncall]
email = "ops@example.com"

[[host]]
name = "web01"
address = "10.0.0.5"
node = "customer-a"
tags = ["linux", "web"]

[[host]]
name = "mq-01"
tags = ["linux"]
vars.port = 5672

[[check]]
name = "disk_root"
use = "linux"
apply_to = 'host.tags contains "linux"'
command = "check_disk -w 20% -c 10% -p /"

[[check]]
name = "queue"
apply_to = 'host.name ~ "^mq-"'
command = "/opt/scripts/queue_depth.sh -H $host.address$ -p $host.vars.port$"
thresholds.depth = { warn = "> 100", crit = "> 500" }
"#;

    #[test]
    fn compiles_the_design_example() {
        let (config, diags) = compile(&[("conf.d/main.toml", EXAMPLE)]);
        assert_eq!(messages(&diags), Vec::<String>::new());
        let ids: Vec<_> = config.services.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["web01/disk_root", "mq-01/disk_root", "mq-01/queue"]);

        let disk = &config.checks[0];
        assert_eq!(disk.retry_interval, Duration::from_secs(15));
        assert_eq!(disk.notify, ["oncall"]);
        assert_eq!(disk.origin.line, 22);

        let queue = &config.checks[1];
        assert_eq!(queue.max_attempts, 3);
        assert!(queue.thresholds["depth"].warn.is_some());
        assert_eq!(
            config.services[2].command,
            Some(watcher_exec::CommandLine::Argv(
                ["/opt/scripts/queue_depth.sh", "-H", "mq-01", "-p", "5672"]
                    .map(String::from)
                    .to_vec()
            ))
        );
        assert_eq!(config.hosts[0].origin.line, 11);
    }

    #[test]
    fn templates_chain_and_merge_deeply() {
        let (config, diags) = compile(&[(
            "a.toml",
            r#"
[template.base]
interval = "5m"
timeout = "10s"
thresholds.load = { warn = "~:5", crit = "~:10" }

[template.strict]
use = "base"
thresholds.load = { crit = "~:8" }

[[host]]
name = "h"

[[check]]
name = "load"
use = ["strict"]
hosts = ["h"]
command = "check_load"
timeout = "20s"
"#,
        )]);
        assert_eq!(messages(&diags), Vec::<String>::new());
        let check = &config.checks[0];
        assert_eq!(check.interval, Duration::from_secs(300));
        assert_eq!(check.retry_interval, Duration::from_secs(60));
        assert_eq!(check.timeout, Duration::from_secs(20));
        let load = &check.thresholds["load"];
        assert_eq!(load.warn.as_ref().unwrap().to_string(), "~:5");
        assert_eq!(load.crit.as_ref().unwrap().to_string(), "~:8");
    }

    #[test]
    fn reports_problems_with_locations() {
        let (_, diags) = compile(&[
            (
                "a.toml",
                r#"
[template.a]
use = "b"

[template.b]
use = "a"

[[host]]
name = "web01"

[[host]]
name = "web01"

[[check]]
name = "x"
use = "a"
hosts = ["web01"]
command = "true"
"#,
            ),
            (
                "b.toml",
                r#"[[check]]
name = "y"
apply_to = 'host.nme == "a"'
command = "check $host.adress$"
interval = "soon"
"#,
            ),
            (
                "c.toml",
                r#"[[check]]
name = "z"
command = "check -H $host.vars.port$"
hosts = ["web01", "nope"]
notify = ["nobody"]
max_attempts = 0
"#,
            ),
            ("d.toml", "[[hosts]]\nname = 1\n"),
        ]);
        assert_eq!(
            messages(&diags),
            [
                "a.toml:5: error: template cycle: a -> b -> a",
                "a.toml:11: error: host \"web01\" is already defined at a.toml:8",
                "b.toml:1: error: invalid duration \"soon\": expected number at 0",
                "c.toml:1: error: check \"z\": max_attempts must be at least 1",
                "c.toml:1: error: check \"z\": unknown contact \"nobody\"",
                "c.toml:1: error: check \"z\": unknown host \"nope\"",
                "d.toml:1: error: unknown field `hosts`, expected one of `template`, `host`, `check`, `contact`, `sink`",
            ]
        );
    }

    #[test]
    fn check_level_validation() {
        let (_, diags) = compile(&[(
            "b.toml",
            r#"[[check]]
name = "y/z"
apply_to = 'host.nme == "a"'
command = "check $host.adress$ 'unterminated"
"#,
        )]);
        assert_eq!(
            messages(&diags),
            [
                "b.toml:1: error: check \"y/z\": apply_to: unknown field \"host.nme\" (expected host.name, host.address, host.node, host.tags or host.vars.<name>) (at column 1)",
                "b.toml:1: error: check \"y/z\": cannot split command: missing closing quote",
                "b.toml:1: error: check \"y/z\": name must not contain '/'",
                "b.toml:1: error: check \"y/z\": unknown macro $host.adress$",
            ]
        );
    }

    #[test]
    fn missing_vars_and_unmatched_checks() {
        let (config, diags) = compile(&[(
            "a.toml",
            r#"
[[host]]
name = "a"
vars.port = 1

[[host]]
name = "b"

[[host]]
name = "c"
enabled = false

[[check]]
name = "port"
apply_to = "true"
command = "check -p $host.vars.port$"

[[check]]
name = "none"
apply_to = 'host.tags contains "nothing"'
command = "true"
"#,
        )]);
        assert_eq!(
            messages(&diags),
            [
                "a.toml:13: error: check \"port\": $host.vars.port$ is not set for host \"b\"",
                "a.toml:18: warning: check \"none\" applies to no enabled host",
            ]
        );
        assert_eq!(config.services.len(), 1);
    }

    #[test]
    fn passive_checks() {
        let (config, diags) = compile(&[(
            "p.toml",
            r#"
[[host]]
name = "a"

[[check]]
name = "backup"
passive = true
freshness = "26h"
hosts = ["a"]

[[check]]
name = "both"
passive = true
command = "true"
hosts = ["a"]

[[check]]
name = "neither"
hosts = ["a"]

[[check]]
name = "fresh"
command = "true"
freshness = "1h"
hosts = ["a"]
"#,
        )]);
        assert_eq!(
            messages(&diags),
            [
                "p.toml:11: error: check \"both\": passive checks have no command",
                "p.toml:17: error: check \"neither\": needs a command, or passive = true",
                "p.toml:21: error: check \"fresh\": freshness only applies to passive checks",
            ]
        );
        let backup = &config.checks[0];
        assert_eq!(backup.command, None);
        assert_eq!(backup.freshness, Some(Duration::from_secs(26 * 3600)));
        assert_eq!(backup.max_attempts, 1);
        assert_eq!(config.services[0].command, None);
        assert_eq!(config.services[0].check_hash.len(), 16);
    }

    #[test]
    fn sinks() {
        let (config, diags) = compile(&[(
            "conf.d/sinks.toml",
            r#"
[[sink]]
name = "central"
type = "postgres"
url = "postgres://watcher@central/watcher"
password_file = "central.pass"

[sink.flush]
mode = "interval"
interval = "5m"

[sink.buffer]
max_size = "2GB"

[[sink]]
name = "live"
type = "postgres"
url = "postgres://x/y"
flush.mode = "realtime"

[[sink]]
name = "bad"
type = "postgres"
url = "mysql://x"
flush.mode = "adaptive"
buffer.max_size = "lots"
buffer.on_full = "block"
"#,
        )]);
        assert_eq!(
            messages(&diags),
            [
                "conf.d/sinks.toml:21: error: sink \"bad\": buffer.on_full block: not available yet",
                "conf.d/sinks.toml:21: error: sink \"bad\": flush.mode adaptive: not available yet",
                "conf.d/sinks.toml:21: error: sink \"bad\": invalid buffer.max_size \"lots\"",
                "conf.d/sinks.toml:21: error: sink \"bad\": url must start with postgres://",
            ]
        );
        let central = &config.sinks[0];
        assert_eq!(
            central.flush,
            crate::Flush::Interval(Duration::from_secs(300))
        );
        assert_eq!(central.max_buffer, 2 << 30);
        assert_eq!(
            central.kind,
            crate::SinkKind::Postgres {
                url: "postgres://watcher@central/watcher".into(),
                password_file: Some(PathBuf::from("conf.d/central.pass")),
            }
        );
        assert_eq!(config.sinks[1].flush, crate::Flush::Realtime);
        assert_eq!(config.sinks[1].max_buffer, crate::Sink::DEFAULT_MAX_BUFFER);
    }

    #[test]
    fn example_sinks_file_is_valid() {
        let text = include_str!("../../../examples/sinks.toml");
        let (config, diags) = compile(&[("examples/sinks.toml", text)]);
        assert_eq!(messages(&diags), Vec::<String>::new());
        assert_eq!(config.sinks.len(), 2);
    }

    #[test]
    fn check_hash_follows_command_and_format() {
        let argv = CommandLine::Argv(vec!["check".into(), "-H".into(), "a".into()]);
        let h = check_hash(Some(&argv), Format::Auto);
        assert_eq!(h, check_hash(Some(&argv.clone()), Format::Auto));
        assert_eq!(h.len(), 16);
        assert_ne!(h, check_hash(Some(&argv), Format::Nagios));
        assert_ne!(
            h,
            check_hash(
                Some(&CommandLine::Argv(vec!["check -H".into(), "a".into()])),
                Format::Auto
            )
        );
        assert_ne!(
            h,
            check_hash(Some(&CommandLine::Shell("check -H a".into())), Format::Auto)
        );
    }

    #[test]
    fn loads_a_directory() {
        let dir = std::env::temp_dir().join(format!("watcher-load-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("hosts.toml"), "[[host]]\nname = \"a\"\n").unwrap();
        std::fs::write(
            dir.join("sub/checks.toml"),
            "[[check]]\nname = \"ok\"\nhosts = [\"a\"]\ncommand = \"true\"\n",
        )
        .unwrap();
        std::fs::write(dir.join(".hidden.toml"), "garbage").unwrap();
        std::fs::write(dir.join("notes.txt"), "garbage").unwrap();

        let loaded = load_dir(&dir).unwrap();
        assert_eq!(loaded.config.files.len(), 2);
        assert_eq!(loaded.config.services[0].id, "a/ok");
        assert!(load_dir(&dir.join("missing")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn host_checks_must_apply_to_the_host() {
        let (config, diags) = compile(&[(
            "h.toml",
            r#"
[template.pinged]
check = "ping"

[[host]]
name = "a"
use = "pinged"

[[host]]
name = "b"
check = "ping"
tags = ["no-ping"]

[[host]]
name = "c"
check = "nope"
graph = "load"

[[check]]
name = "ping"
apply_to = 'not (host.tags contains "no-ping")'
command = "check_ping -H $host.address$"
"#,
        )]);
        assert_eq!(
            messages(&diags),
            [
                "h.toml:9: error: host \"b\": check \"ping\" does not apply to it",
                "h.toml:14: error: host \"c\": unknown check \"nope\"",
                "h.toml:14: warning: host \"c\": graph: no service \"load\" on it",
            ]
        );
        let a = &config.hosts[0];
        assert_eq!(config.host_check(a).unwrap().id, "a/ping");
        assert_eq!(a.graph_check(), Some("ping"));
        assert!(config.is_host_check(config.host_check(a).unwrap()));
    }

    #[test]
    fn flap_settings() {
        let check = |extra: &str| {
            let text = format!(
                "[[host]]\nname = \"a\"\n[[check]]\nname = \"x\"\nhosts = [\"a\"]\ncommand = \"true\"\n{extra}"
            );
            let (config, diags) = compile(&[("c.toml", text.as_str())]);
            (config.checks.first().map(|c| c.flap), messages(&diags))
        };
        assert_eq!(check(""), (Some(Some(FlapThresholds::DEFAULT)), vec![]));
        assert_eq!(check("flap_detection = false"), (Some(None), vec![]));
        assert_eq!(
            check("flap_low = 10\nflap_high = 30").0,
            Some(Some(FlapThresholds {
                low: 10.0,
                high: 30.0
            }))
        );
        assert_eq!(
            check("flap_low = 40").1,
            ["c.toml:3: error: check \"x\": flap_low must not be above flap_high"]
        );
        assert_eq!(
            check("flap_high = 120").1,
            ["c.toml:3: error: check \"x\": flap_low and flap_high are percentages (0 to 100)"]
        );
    }
}
