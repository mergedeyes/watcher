//! Configuration for Project Watcher: the bootstrap file (`watcher.toml`)
//! and the object files (`conf.d/**/*.toml`) with hosts, checks, templates
//! and contacts.
//!
//! Loading compiles the object files into a [`Config`]: templates are merged
//! in, `apply_to` rules are evaluated and every host × check pair becomes a
//! [`Service`] with its command macros expanded.

mod bootstrap;
mod expr;
mod load;
mod macros;

pub use bootstrap::{Bootstrap, BootstrapError, Database, Role, Smtp, SmtpTls};
pub use expr::{Expr, ExprError};
pub use load::load_dir;

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;
use watcher_core::{Format, ThresholdSpec};
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

#[derive(Debug, Clone, PartialEq)]
pub struct Host {
    pub name: String,
    /// Defaults to the name.
    pub address: String,
    /// The node that runs this host's checks; `None` for the local instance.
    pub node: Option<String>,
    pub tags: Vec<String>,
    pub vars: BTreeMap<String, String>,
    pub enabled: bool,
    pub origin: Origin,
}

#[derive(Debug, Clone)]
pub struct Check {
    pub name: String,
    /// Command text before macro expansion; `None` for passive checks,
    /// whose results are pushed through the API.
    pub command: Option<String>,
    pub shell: bool,
    pub format: Format,
    pub interval: Duration,
    pub retry_interval: Duration,
    pub max_attempts: u32,
    pub timeout: Duration,
    /// Passive checks only: a missing result for this long turns the
    /// service UNKNOWN.
    pub freshness: Option<Duration>,
    pub thresholds: BTreeMap<String, ThresholdSpec>,
    /// Contact names.
    pub notify: Vec<String>,
    pub vars: BTreeMap<String, String>,
    pub apply_to: Option<Expr>,
    /// Hosts listed explicitly, in addition to `apply_to` matches.
    pub hosts: Vec<String>,
    pub enabled: bool,
    pub origin: Origin,
}

impl Check {
    pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(60);
    /// For active checks; passive checks default to 1.
    pub const DEFAULT_MAX_ATTEMPTS: u32 = 3;
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

    #[cfg(test)]
    pub(crate) fn for_tests(name: &str) -> Self {
        Check {
            name: name.into(),
            command: None,
            shell: false,
            format: Format::Auto,
            interval: Self::DEFAULT_INTERVAL,
            retry_interval: Self::DEFAULT_INTERVAL,
            max_attempts: Self::DEFAULT_MAX_ATTEMPTS,
            timeout: Self::DEFAULT_TIMEOUT,
            freshness: None,
            thresholds: BTreeMap::new(),
            notify: Vec::new(),
            vars: BTreeMap::new(),
            apply_to: None,
            hosts: Vec::new(),
            enabled: true,
            origin: Origin::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Contact {
    pub name: String,
    pub email: Vec<String>,
    pub webhook: Option<String>,
    pub origin: Origin,
}

/// A check applied to a host.
#[derive(Debug, Clone, PartialEq)]
pub struct Service {
    /// `host/check`.
    pub id: String,
    /// Index into [`Config::hosts`].
    pub host: usize,
    /// Index into [`Config::checks`].
    pub check: usize,
    /// The command with macros expanded for this host; `None` if passive.
    pub command: Option<CommandLine>,
    /// Stable hash of what determines the meaning of the data (command and
    /// output format). A change marks where data from a new check begins.
    pub check_hash: String,
}

/// The compiled object configuration.
#[derive(Debug, Clone, Default)]
pub struct Config {
    pub files: Vec<PathBuf>,
    pub hosts: Vec<Host>,
    pub checks: Vec<Check>,
    pub contacts: Vec<Contact>,
    pub services: Vec<Service>,
}

impl Config {
    pub fn host(&self, service: &Service) -> &Host {
        &self.hosts[service.host]
    }

    pub fn check(&self, service: &Service) -> &Check {
        &self.checks[service.check]
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
