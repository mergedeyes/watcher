//! Configuration for Project Watcher: the bootstrap file (`watcher.toml`)
//! and the object files (`conf.d/**/*.toml`) with checks, services, hosts,
//! groups, contacts, notify rules and sinks.
//!
//! Loading compiles the object files into a [`Config`]: hosts get services
//! listed on them, on their host groups and in the service groups of
//! either; every service is resolved layer by layer (host > host group >
//! service > check > defaults) with the source of each value recorded, and
//! command macros are expanded.

mod bootstrap;
pub mod check_dirs;
pub mod diff;
pub mod edit;
mod expr;
pub mod files;
mod load;
mod macros;
pub mod schedule;
mod select;
pub mod tree;

pub use bootstrap::{
    Bootstrap, BootstrapError, ConfigMode, Database, InstanceRole, Smtp, SmtpTls, Tls,
};
pub use check_dirs::CheckDirs;
pub use expr::{Expr, ExprError};
pub use load::{load_dir, load_dir_with};
pub use schedule::Window;
pub use select::{HostSelector, ServiceSelector};

use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;
use watcher_core::{FlapThresholds, Format, State, ThresholdSpec};
use watcher_exec::CommandLine;

/// Where an object or problem was defined.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Origin {
    pub file: PathBuf,
    /// 1-based; 0 when unknown.
    pub line: usize,
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line > 0 {
            write!(f, "{}:{}", self.file.display(), self.line)
        } else {
            write!(f, "{}", self.file.display())
        }
    }
}

/// A variable: text (numbers and booleans are kept as text), or a table,
/// e.g. the entries `for_each` iterates over.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Var {
    Text(String),
    Table(BTreeMap<String, Var>),
}

impl Var {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Var::Text(s) => Some(s),
            Var::Table(_) => None,
        }
    }
}

pub type Vars = BTreeMap<String, Var>;

/// The var at a dotted path like `site.klarsocial`.
pub fn var_at<'v>(vars: &'v Vars, path: &str) -> Option<&'v Var> {
    let mut parts = path.split('.');
    let mut var = vars.get(parts.next()?)?;
    for part in parts {
        let Var::Table(t) = var else { return None };
        var = t.get(part)?;
    }
    Some(var)
}

/// The layer a value comes from, most specific last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    Default,
    Check,
    Service,
    HostGroup,
    Host,
    ForEach,
    HostService,
}

/// Where a value came from: the layer, the object and the place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub layer: Layer,
    /// The check, service, host group or host; for `ForEach`
    /// `host.vars.<var>.<key>`, for `HostService` `host.service`.
    pub name: String,
    pub origin: Origin,
}

/// Why a host has a service, or an object is in a group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    /// Listed by name, at `origin` (the host, group, service or contact that
    /// lists it).
    Listed { by: String, origin: Origin },
    /// A pattern or selector matched; `matched` says which.
    Selector { origin: Origin, matched: String },
}

/// A service assignment or group membership, with the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assigned {
    pub name: String,
    pub reason: Reason,
}

/// What to run and how to judge it: the reusable part of a service.
#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    pub name: String,
    pub title: Option<String>,
    /// Command text before macro expansion; `None` for passive checks.
    pub command: Option<String>,
    pub origin: Origin,
}

/// A service as defined on its own (`[service.<name>]`), before it is
/// assigned to hosts.
#[derive(Debug, Clone, PartialEq)]
pub struct ServiceObject {
    pub name: String,
    pub title: Option<String>,
    /// The check it runs; `None` for an inline command or passive service.
    pub check: Option<String>,
    /// Service groups it lists.
    pub groups: Vec<String>,
    pub origin: Origin,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Host {
    pub name: String,
    /// Defaults to the name.
    pub address: String,
    /// The node that runs this host's checks; `None` for the local instance.
    pub node: Option<String>,
    /// Host groups.
    pub groups: Vec<Assigned>,
    /// The host's own vars over those of its host groups.
    pub vars: Vars,
    /// The service whose result is the host state (UP/DOWN); without one
    /// the host counts as UP.
    pub host_check: Option<String>,
    /// The service whose graphs the host page shows; defaults to
    /// `host_check`.
    pub graph: Option<String>,
    pub enabled: bool,
    pub origin: Origin,
    /// Where `address`, `node`, `host_check`, `graph` and each `vars.<x>`
    /// came from.
    pub sources: BTreeMap<String, Source>,
}

impl Host {
    /// The service whose graphs show on the host page: `graph`, or the host
    /// check.
    pub fn graph_check(&self) -> Option<&str> {
        self.graph.as_deref().or(self.host_check.as_deref())
    }

    pub fn in_group(&self, group: &str) -> bool {
        self.groups.iter().any(|g| g.name == group)
    }
}

/// How a service runs and is evaluated, after all layers.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub shell: bool,
    pub passive: bool,
    pub format: Format,
    pub interval: Duration,
    pub retry_interval: Duration,
    pub max_attempts: u32,
    pub timeout: Duration,
    /// Passive services only: a missing result for this long turns the
    /// service UNKNOWN.
    pub freshness: Option<Duration>,
    /// Flap detection thresholds; `None` when turned off.
    pub flap: Option<FlapThresholds>,
    pub thresholds: BTreeMap<String, ThresholdSpec>,
    /// Show graphs of its metrics.
    pub graphs: bool,
}

impl Settings {
    pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(60);
    /// For active services; passive ones default to 1.
    pub const DEFAULT_MAX_ATTEMPTS: u32 = 3;
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
}

/// A check on a host.
#[derive(Debug, Clone, PartialEq)]
pub struct Service {
    /// `host/name`.
    pub id: String,
    pub name: String,
    /// Index into [`Config::hosts`].
    pub host: usize,
    /// The check it runs, if it uses one (not for inline commands).
    pub check: Option<String>,
    /// The service object it is made from; `None` for a host's one-off
    /// service.
    pub object: Option<String>,
    /// Why the host has it: listed on the host or a host group, directly or
    /// in a service group. Empty for one-off services.
    pub assigned: Vec<Assigned>,
    /// The `for_each` var and entry key it was made from.
    pub for_each: Option<(String, String)>,
    /// Command text before macro expansion.
    pub command_text: Option<String>,
    /// The command with macros expanded; `None` if passive.
    pub command: Option<CommandLine>,
    pub settings: Settings,
    /// Vars after all layers (text only), as used by the command.
    pub vars: BTreeMap<String, String>,
    /// Service groups.
    pub groups: Vec<Assigned>,
    /// Stable hash of what determines the meaning of the data (command and
    /// output format). A change marks where data from a new check begins.
    pub check_hash: String,
    /// Where the service is defined: the service object, or the host's
    /// `service.<name>`.
    pub origin: Origin,
    /// Where each setting and `vars.<x>` came from, by key.
    pub sources: BTreeMap<String, Source>,
    /// Every layer that contributed, least specific first.
    pub layers: Vec<Source>,
}

impl Service {
    pub fn in_group(&self, group: &str) -> bool {
        self.groups.iter().any(|g| g.name == group)
    }
}

/// A host, service or contact group.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub name: String,
    pub title: Option<String>,
    /// As written: host names and patterns (host groups), contacts
    /// (contact groups); empty for service groups.
    pub members: Vec<String>,
    /// Services it lists: those its hosts get (host groups), its members
    /// (service groups).
    pub services: Vec<String>,
    /// Service groups whose services its hosts get (host groups).
    pub servicegroups: Vec<String>,
    pub origin: Origin,
}

/// Hosts that share planned downtimes: listed by name or pattern, or in
/// a host group.
#[derive(Debug, Clone, PartialEq)]
pub struct DowntimeGroup {
    pub name: String,
    pub title: Option<String>,
    /// Host names and patterns, as written.
    pub members: Vec<String>,
    pub hostgroups: Vec<String>,
    /// Recurring or one-off windows, in local time.
    pub schedule: Vec<Window>,
    pub origin: Origin,
}

impl DowntimeGroup {
    pub fn has_host(&self, host: &Host) -> bool {
        self.hostgroups.iter().any(|g| host.in_group(g))
            || self
                .members
                .iter()
                .any(|m| select::Glob::new(m).is_match(&host.name))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Contact {
    pub name: String,
    pub email: Vec<String>,
    pub webhook: Option<String>,
    /// Contact groups.
    pub groups: Vec<Assigned>,
    pub origin: Origin,
}

/// Which notifications a rule takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotifyOn {
    pub problem: bool,
    pub recovery: bool,
    pub flapping: bool,
    /// A downtime starts or ends.
    pub downtime: bool,
}

/// Who gets which alerts. Every matching rule notifies.
#[derive(Debug, Clone)]
pub struct NotifyRule {
    pub name: Option<String>,
    pub contacts: Vec<String>,
    pub contact_groups: Vec<String>,
    pub hosts: Option<HostSelector>,
    /// With services set, the rule only takes service alerts.
    pub services: Option<ServiceSelector>,
    /// Problem states the rule takes (`warning`, `critical`, `unknown`,
    /// `down`); all of them when not set.
    pub states: Vec<String>,
    pub on: NotifyOn,
    pub enabled: bool,
    pub origin: Origin,
}

impl NotifyRule {
    /// The name, or where it is defined.
    pub fn label(&self) -> String {
        match &self.name {
            Some(name) => name.clone(),
            None => format!("rule at {}", self.origin),
        }
    }

    /// Whether the rule takes `state` (as `ok`, `warning`, ..., `up`, `down`).
    pub fn takes_state(&self, state: &str) -> bool {
        if self.states.is_empty() {
            return !matches!(state, "ok" | "up");
        }
        self.states.iter().any(|s| s == state)
    }
}

/// The kind of a notification, for [`Config::recipients`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alert {
    Problem,
    /// Back to OK/UP from the given problem state.
    Recovery,
    Flapping,
    /// A downtime starts or ends.
    Downtime,
}

/// A destination that receives a copy of all results, through a durable
/// local buffer.
#[derive(Debug, Clone, PartialEq)]
pub struct Sink {
    pub name: String,
    pub kind: SinkKind,
    pub flush: Flush,
    /// Size of the local buffer in bytes; the oldest data is dropped beyond it.
    pub max_buffer: u64,
    pub origin: Origin,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SinkKind {
    /// Another PostgreSQL with the Watcher schema (created on connect).
    Postgres {
        url: String,
        /// Password kept out of the URL, read when connecting.
        password_file: Option<PathBuf>,
    },
}

/// When a sink sends buffered data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flush {
    /// As soon as results arrive (batched over about a second).
    Realtime,
    Interval(Duration),
}

impl Sink {
    pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(60);
    pub const DEFAULT_MAX_BUFFER: u64 = 1 << 30;
}

/// The compiled object configuration.
#[derive(Debug, Clone, Default)]
pub struct Config {
    pub files: Vec<PathBuf>,
    pub checks: Vec<Check>,
    /// Services as defined, before they are assigned.
    pub service_objects: Vec<ServiceObject>,
    pub hosts: Vec<Host>,
    /// Services on hosts.
    pub services: Vec<Service>,
    pub hostgroups: Vec<Group>,
    pub servicegroups: Vec<Group>,
    pub contacts: Vec<Contact>,
    pub contactgroups: Vec<Group>,
    pub downtimegroups: Vec<DowntimeGroup>,
    pub notify: Vec<NotifyRule>,
    pub sinks: Vec<Sink>,
}

impl Config {
    pub fn host(&self, service: &Service) -> &Host {
        &self.hosts[service.host]
    }

    /// The service that gives `host` its UP/DOWN state.
    pub fn host_check(&self, host: &Host) -> Option<&Service> {
        let name = host.host_check.as_deref()?;
        self.services
            .iter()
            .find(|s| s.name == name && self.host(s).name == host.name)
    }

    /// Whether `service` gives its host its state.
    pub fn is_host_check(&self, service: &Service) -> bool {
        self.host(service).host_check.as_deref() == Some(service.name.as_str())
    }

    pub fn service(&self, id: &str) -> Option<&Service> {
        self.services.iter().find(|s| s.id == id)
    }

    /// The contacts of a contact group: listed in the group or listing it.
    pub fn group_contacts(&self, group: &str) -> Vec<&Contact> {
        self.contacts
            .iter()
            .filter(|c| c.groups.iter().any(|g| g.name == group))
            .collect()
    }

    /// The notify rules that match an alert about `service`, with why. A
    /// host alert (from a host check) only matches rules without
    /// `services`.
    pub fn matching_rules(
        &self,
        service: &Service,
        host_alert: bool,
    ) -> Vec<(&NotifyRule, String)> {
        let host = self.host(service);
        self.notify
            .iter()
            .filter(|r| r.enabled)
            .filter_map(|rule| {
                let mut why = Vec::new();
                if let Some(sel) = &rule.hosts {
                    why.push(format!("host: {}", sel.matches(host)?));
                }
                match (&rule.services, host_alert) {
                    (Some(_), true) => return None,
                    (Some(sel), false) => {
                        why.push(format!("service: {}", sel.matches(service, host)?))
                    }
                    (None, _) => {}
                }
                if why.is_empty() {
                    why.push("all".into());
                }
                Some((rule, why.join(", ")))
            })
            .collect()
    }

    /// Who gets an alert: the contacts of every matching rule that takes
    /// it, each once. `state` is the state reached (for a recovery, the
    /// problem state it recovers from), as `warning`, `down`, ...
    pub fn recipients(
        &self,
        service: &Service,
        host_alert: bool,
        alert: Alert,
        state: &str,
    ) -> Vec<&Contact> {
        let mut names: Vec<&str> = Vec::new();
        for (rule, _) in self.matching_rules(service, host_alert) {
            let takes = match alert {
                Alert::Problem => rule.on.problem && rule.takes_state(state),
                Alert::Recovery => rule.on.recovery && rule.takes_state(state),
                Alert::Flapping => rule.on.flapping,
                Alert::Downtime => rule.on.downtime,
            };
            if !takes {
                continue;
            }
            names.extend(rule.contacts.iter().map(String::as_str));
            for group in &rule.contact_groups {
                names.extend(self.group_contacts(group).iter().map(|c| c.name.as_str()));
            }
        }
        let mut out: Vec<&Contact> = Vec::new();
        for name in names {
            if let Some(c) = self.contacts.iter().find(|c| c.name == name)
                && !out.iter().any(|o| o.name == name)
            {
                out.push(c);
            }
        }
        out
    }
}

impl Config {
    /// Who hears about a downtime of `host`: through its host check if it
    /// has one, else the rules without `services` that match the host.
    pub fn host_downtime_recipients(&self, host: &Host) -> Vec<&Contact> {
        if let Some(hc) = self.host_check(host) {
            return self.recipients(hc, true, Alert::Downtime, "up");
        }
        let mut names: Vec<&str> = Vec::new();
        for rule in &self.notify {
            let takes = rule.enabled
                && rule.on.downtime
                && rule.services.is_none()
                && rule
                    .hosts
                    .as_ref()
                    .is_none_or(|s| s.matches(host).is_some());
            if !takes {
                continue;
            }
            names.extend(rule.contacts.iter().map(String::as_str));
            for group in &rule.contact_groups {
                names.extend(self.group_contacts(group).iter().map(|c| c.name.as_str()));
            }
        }
        let mut out: Vec<&Contact> = Vec::new();
        for name in names {
            if let Some(c) = self.contacts.iter().find(|c| c.name == name)
                && !out.iter().any(|o| o.name == name)
            {
                out.push(c);
            }
        }
        out
    }
}

/// `ok`, `warning`, ... for a state, or `up`/`down` for a host alert.
pub fn state_name(state: State, host_alert: bool) -> &'static str {
    if host_alert {
        match watcher_core::HostState::from_check(state) {
            watcher_core::HostState::Up => "up",
            watcher_core::HostState::Down => "down",
        }
    } else {
        match state {
            State::Ok => "ok",
            State::Warning => "warning",
            State::Critical => "critical",
            State::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Diagnostic {
    pub origin: Option<Origin>,
    pub severity: Severity,
    pub message: String,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let level = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        match &self.origin {
            Some(origin) => write!(f, "{origin}: {level}: {}", self.message),
            None => write!(f, "{level}: {}", self.message),
        }
    }
}

/// A successfully loaded configuration and any warnings.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub config: Config,
    pub warnings: Vec<Diagnostic>,
}
