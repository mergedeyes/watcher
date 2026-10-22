//! JSON shapes of the API.

use crate::LiveService;
use serde::Serialize;
use std::collections::BTreeMap;
use std::time::SystemTime;
use watcher_config::{Config, Host, Service};
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
    /// `None` (null) while pending: no result yet.
    pub state: Option<State>,
    pub state_type: Option<StateType>,
    pub attempt: Option<u32>,
    pub max_attempts: u32,
    pub last_check: Option<String>,
    pub last_change: Option<String>,
    pub output: String,
    pub passive: bool,
    /// The state changes too often; problem notifications are held back.
    pub flapping: bool,
}

impl ServiceView {
    pub fn new(config: &Config, service: &Service, live: Option<&LiveService>) -> Self {
        let check = config.check(service);
        ServiceView {
            host: config.host(service).name.clone(),
            service: check.name.clone(),
            state: live.map(|l| l.state.state),
            state_type: live.map(|l| l.state.state_type),
            attempt: live.map(|l| l.state.attempt),
            max_attempts: check.max_attempts,
            last_check: live.map(|l| time(l.state.last_check)),
            last_change: live.map(|l| time(l.state.last_change)),
            output: live.map(|l| l.output.clone()).unwrap_or_default(),
            passive: check.command.is_none(),
            flapping: live.is_some_and(|l| l.state.flapping),
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
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckView {
    /// `None` for passive checks.
    pub command: Option<String>,
    pub interval_s: f64,
    pub retry_interval_s: f64,
    pub timeout_s: f64,
    pub freshness_s: Option<f64>,
    /// Flap detection thresholds in percent; `None` when off.
    pub flap_low: Option<f64>,
    pub flap_high: Option<f64>,
    /// Where the check is defined, `file:line`.
    pub origin: String,
}

impl ServiceDetail {
    pub fn new(config: &Config, service: &Service, live: Option<&LiveService>) -> Self {
        let check = config.check(service);
        ServiceDetail {
            view: ServiceView::new(config, service, live),
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
                command: service.command.as_ref().map(ToString::to_string),
                interval_s: check.interval.as_secs_f64(),
                retry_interval_s: check.retry_interval.as_secs_f64(),
                timeout_s: check.timeout.as_secs_f64(),
                freshness_s: check.freshness.map(|f| f.as_secs_f64()),
                flap_low: check.flap.map(|f| f.low),
                flap_high: check.flap.map(|f| f.high),
                origin: check.origin.to_string(),
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
    pub tags: Vec<String>,
    pub vars: BTreeMap<String, String>,
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
    /// The worst state of the host's services; `None` if all are pending.
    pub worst: Option<State>,
    pub services: Counts,
}

impl HostView {
    /// `services` are the host's services, including its host check.
    pub fn new(host: &Host, services: &[&ServiceView]) -> Self {
        let mut counts = Counts::default();
        let mut worst: Option<State> = None;
        for s in services {
            counts.add(s.state);
            if let Some(state) = s.state {
                worst = Some(worst.map_or(state, |w| w.worst(state)));
            }
        }
        let check = host
            .check
            .as_ref()
            .and_then(|name| services.iter().find(|s| s.service == *name));
        let state = match (&host.check, check) {
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
            tags: host.tags.clone(),
            vars: host.vars.clone(),
            enabled: host.enabled,
            state,
            state_type,
            attempt: check.and_then(|v| v.attempt),
            max_attempts: check.map(|v| v.max_attempts),
            last_check: check.and_then(|v| v.last_check.clone()),
            last_change: check.and_then(|v| v.last_change.clone()),
            output: check.map(|v| v.output.clone()).unwrap_or_default(),
            flapping: check.is_some_and(|v| v.flapping),
            check: host.check.clone(),
            worst,
            services: counts,
        }
    }
}
