//! JSON shapes of the API.

use crate::LiveService;
use crate::ops::Status;
use serde::Serialize;

use std::time::SystemTime;
use watcher_config::{Assigned, Config, Host, Reason, Service, Vars};
use watcher_core::{HostState, MetricResult, State, StateType};

/// RFC 3339 with milliseconds, e.g. `2026-10-22T10:00:00.123Z`.
pub fn time(t: SystemTime) -> String {
    humantime::format_rfc3339_millis(t).to_string()
}

/// A service in lists and stream updates.
#[derive(Debug, Clone, Serialize)]
pub struct ServiceView {
    pub host: String,
    pub service: String,
    /// For display; `None` shows the name.
    pub title: Option<String>,
    /// `None` (null) while pending: no result yet.
    pub state: Option<State>,
    pub state_type: Option<StateType>,
    pub attempt: Option<u32>,
    pub max_attempts: u32,
    pub last_check: Option<String>,
    pub last_change: Option<String>,
    pub output: String,
    pub passive: bool,
    /// Show graphs of its metrics.
    pub graphs: bool,
    /// The state changes too often; problem notifications are held back.
    pub flapping: bool,
    /// Service groups.
    pub groups: Vec<String>,
    /// The problem is acknowledged.
    pub acknowledged: bool,
    /// Notifications are off for it or its host.
    pub muted: bool,
    /// Active checks run.
    pub checks_enabled: bool,
    /// In a downtime now.
    pub downtime: bool,
}

fn names(list: &[Assigned]) -> Vec<String> {
    list.iter().map(|a| a.name.clone()).collect()
}

impl ServiceView {
    pub fn new(
        config: &Config,
        service: &Service,
        live: Option<&LiveService>,
        status: &Status,
    ) -> Self {
        ServiceView {
            host: config.host(service).name.clone(),
            service: service.name.clone(),
            title: service.title.clone(),
            state: live.map(|l| l.state.state),
            state_type: live.map(|l| l.state.state_type),
            attempt: live.map(|l| l.state.attempt),
            max_attempts: service.settings.max_attempts,
            last_check: live.map(|l| time(l.state.last_check)),
            last_change: live.map(|l| time(l.state.last_change)),
            output: live.map(|l| l.output.clone()).unwrap_or_default(),
            passive: service.settings.passive,
            graphs: service.settings.graphs,
            flapping: live.is_some_and(|l| l.state.flapping),
            groups: names(&service.groups),
            acknowledged: status.acknowledged,
            muted: status.muted,
            checks_enabled: status.checks,
            downtime: !status.downtime.is_empty(),
        }
    }
}

/// One service with everything known about it.
#[derive(Debug, Clone, Serialize)]
pub struct ServiceDetail {
    #[serde(flatten)]
    pub view: ServiceView,
    pub last_hard_change: Option<String>,
    pub long_output: String,
    /// How long the last check ran, in milliseconds.
    pub duration_ms: Option<f64>,
    /// Metrics of the last result; empty after a restart until the next check.
    pub metrics: Vec<MetricResult>,
    /// Weighted share of state changes in the last 21 results, 0 to 100.
    pub percent_change: Option<f64>,
    /// This service gives its host its UP/DOWN state.
    pub host_check: bool,
    pub check: CheckView,
    /// The downtimes it is in, e.g. `hostgroup linux`.
    pub downtimes: Vec<String>,
    /// Acknowledgement, mute and disabled checks set on it or its host.
    pub flags: Vec<watcher_store::Flag>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckView {
    /// The check it runs; `None` for an inline command.
    pub name: Option<String>,
    /// The service object it is made from (`disk` for `disk_root` made
    /// with `for_each`); `None` for a host's one-off service.
    pub service: Option<String>,
    /// Why the host has it: `hostgroup linux`, `servicegroup basics on
    /// host web01`, ...
    pub assigned: Vec<String>,
    /// `None` for passive checks.
    pub command: Option<String>,
    pub interval_s: f64,
    pub retry_interval_s: f64,
    pub timeout_s: f64,
    pub freshness_s: Option<f64>,
    /// Flap detection thresholds in percent; `None` when off.
    pub flap_low: Option<f64>,
    pub flap_high: Option<f64>,
    /// Where the service is defined (service or host), `file:line`.
    pub origin: String,
}

impl ServiceDetail {
    pub fn new(
        config: &Config,
        service: &Service,
        live: Option<&LiveService>,
        status: &Status,
        flags: Vec<watcher_store::Flag>,
    ) -> Self {
        let settings = &service.settings;
        ServiceDetail {
            view: ServiceView::new(config, service, live, status),
            downtimes: status.downtime.clone(),
            flags,
            last_hard_change: live.map(|l| time(l.state.last_hard_change)),
            long_output: live.map(|l| l.long_output.clone()).unwrap_or_default(),
            duration_ms: live
                .filter(|l| l.result.is_some())
                .map(|l| l.duration.as_secs_f64() * 1000.0),
            metrics: live
                .and_then(|l| l.result.as_ref())
                .map(|r| r.metrics.clone())
                .unwrap_or_default(),
            percent_change: live.map(|l| l.state.history.percent_change()),
            host_check: config.is_host_check(service),
            check: CheckView {
                name: service.check.clone(),
                service: service.object.clone(),
                assigned: service
                    .assigned
                    .iter()
                    .map(|a| match &a.reason {
                        Reason::Listed { by, .. } => by.clone(),
                        Reason::Selector { matched, .. } => matched.clone(),
                    })
                    .collect(),
                command: service.command.as_ref().map(ToString::to_string),
                interval_s: settings.interval.as_secs_f64(),
                retry_interval_s: settings.retry_interval.as_secs_f64(),
                timeout_s: settings.timeout.as_secs_f64(),
                freshness_s: settings.freshness.map(|f| f.as_secs_f64()),
                flap_low: settings.flap.map(|f| f.low),
                flap_high: settings.flap.map(|f| f.high),
                origin: service.origin.to_string(),
            },
        }
    }
}

/// Number of services per state.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Counts {
    pub ok: usize,
    pub warning: usize,
    pub critical: usize,
    pub unknown: usize,
    pub pending: usize,
}

impl Counts {
    pub fn add(&mut self, state: Option<State>) {
        match state {
            Some(State::Ok) => self.ok += 1,
            Some(State::Warning) => self.warning += 1,
            Some(State::Critical) => self.critical += 1,
            Some(State::Unknown) => self.unknown += 1,
            None => self.pending += 1,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct HostView {
    pub name: String,
    pub address: String,
    pub node: Option<String>,
    pub vars: Vars,
    /// Host groups.
    pub groups: Vec<String>,
    pub enabled: bool,
    /// From the host check; UP without one. `None` (null) while the host
    /// check is pending.
    pub state: Option<HostState>,
    pub state_type: Option<StateType>,
    pub attempt: Option<u32>,
    pub max_attempts: Option<u32>,
    pub last_check: Option<String>,
    pub last_change: Option<String>,
    pub output: String,
    pub flapping: bool,
    /// Name of the host check, if there is one.
    pub check: Option<String>,
    /// The service whose graphs the host page shows: `graph`, or the host
    /// check.
    pub graph: Option<String>,
    /// The worst state of the host's services; `None` if all are pending.
    pub worst: Option<State>,
    pub services: Counts,
    /// The host problem (host check) is acknowledged.
    pub acknowledged: bool,
    /// Notifications are off for the host.
    pub muted: bool,
    /// Active checks of the host run.
    pub checks_enabled: bool,
    /// In a downtime now.
    pub downtime: bool,
}

/// Host-level flags, for [`HostView::new`].
#[derive(Debug, Clone, Default)]
pub struct HostFlags {
    pub acknowledged: bool,
    pub muted: bool,
    pub checks_enabled: bool,
    pub downtime: bool,
}

impl HostView {
    /// `services` are the host's services, including its host check.
    pub fn new(host: &Host, services: &[&ServiceView], flags: &HostFlags) -> Self {
        let mut counts = Counts::default();
        let mut worst: Option<State> = None;
        for s in services {
            counts.add(s.state);
            if let Some(state) = s.state {
                worst = Some(worst.map_or(state, |w| w.worst(state)));
            }
        }
        let check = host
            .host_check
            .as_ref()
            .and_then(|name| services.iter().find(|s| s.service == *name));
        let state = match (&host.host_check, check) {
            (None, _) => Some(HostState::Up),
            (Some(_), view) => view.and_then(|v| v.state).map(HostState::from_check),
        };
        // UP is confirmed; a DOWN takes its attempts from the check.
        let state_type = match state {
            Some(HostState::Up) => Some(StateType::Hard),
            _ => check.and_then(|v| v.state_type),
        };
        HostView {
            name: host.name.clone(),
            address: host.address.clone(),
            node: host.node.clone(),
            vars: host.vars.clone(),
            groups: names(&host.groups),
            enabled: host.enabled,
            state,
            state_type,
            attempt: check.and_then(|v| v.attempt),
            max_attempts: check.map(|v| v.max_attempts),
            last_check: check.and_then(|v| v.last_check.clone()),
            last_change: check.and_then(|v| v.last_change.clone()),
            output: check.map(|v| v.output.clone()).unwrap_or_default(),
            flapping: check.is_some_and(|v| v.flapping),
            check: host.host_check.clone(),
            graph: host.graph_check().map(String::from),
            worst,
            services: counts,
            acknowledged: flags.acknowledged || check.is_some_and(|c| c.acknowledged),
            muted: flags.muted,
            checks_enabled: flags.checks_enabled,
            downtime: flags.downtime,
        }
    }
}
