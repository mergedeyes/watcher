//! Host and service lists filtered, sorted and paged on the server, and
//! status summaries. Lists are built from the live state in one pass; views
//! are only made for the rows of the requested page.

use crate::api::ApiError;
use crate::view::{Counts, HostFlags, HostView, ServiceView};
use crate::{Live, LiveService};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::time::SystemTime;
use watcher_config::{Config, Host, Service};
use watcher_core::{HostState, State, StateType};
use watcher_store::FlagKind;

/// Compares like a person sorts: digit runs by value (`web2` before
/// `web10`), letters ignoring case.
pub(crate) fn natural(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a, b);
    loop {
        match (x.chars().next(), y.chars().next()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let n = x.find(|c: char| !c.is_ascii_digit()).unwrap_or(x.len());
                let m = y.find(|c: char| !c.is_ascii_digit()).unwrap_or(y.len());
                let (p, q) = (
                    x[..n].trim_start_matches('0'),
                    y[..m].trim_start_matches('0'),
                );
                let order = p.len().cmp(&q.len()).then_with(|| p.cmp(q));
                if order != Ordering::Equal {
                    return order;
                }
                (x, y) = (&x[n..], &y[m..]);
            }
            (Some(c), Some(d)) => {
                let order = c.to_lowercase().cmp(d.to_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                (x, y) = (&x[c.len_utf8()..], &y[d.len_utf8()..]);
            }
        }
    }
}

/// `needle` is lowercase already.
fn contains(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(needle)
}

/// Sort order of service states, as in the UI: OK, PENDING, WARNING,
/// UNKNOWN, CRITICAL.
fn severity(state: Option<State>) -> u8 {
    match state {
        Some(State::Ok) => 0,
        None => 1,
        Some(State::Warning) => 2,
        Some(State::Unknown) => 3,
        Some(State::Critical) => 4,
    }
}

/// UP, PENDING, DOWN.
fn host_severity(state: Option<HostState>) -> u8 {
    match state {
        Some(HostState::Up) => 0,
        None => 1,
        Some(HostState::Down) => 2,
    }
}

/// Comma-separated service states, `pending` for no result yet.
fn parse_states(list: &str) -> Result<Vec<Option<State>>, ApiError> {
    list.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| match s {
            "pending" => Ok(None),
            other => State::parse(other)
                .map(Some)
                .ok_or_else(|| ApiError::BadRequest(format!("unknown state {other:?}"))),
        })
        .collect()
}

fn parse_host_states(list: &str) -> Result<Vec<Option<HostState>>, ApiError> {
    list.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| match s {
            "up" => Ok(Some(HostState::Up)),
            "down" => Ok(Some(HostState::Down)),
            "pending" => Ok(None),
            other => Err(ApiError::BadRequest(format!(
                "unknown host state {other:?}"
            ))),
        })
        .collect()
}

fn sort_key<K: Copy>(value: Option<&str>, keys: &[(&str, K)]) -> Result<Option<K>, ApiError> {
    let Some(value) = value.filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    keys.iter()
        .find(|(name, _)| *name == value)
        .map(|(_, k)| Some(*k))
        .ok_or_else(|| {
            let names: Vec<&str> = keys.iter().map(|(n, _)| *n).collect();
            ApiError::BadRequest(format!("sort: one of {}", names.join(", ")))
        })
}

fn page<T>(rows: Vec<T>, offset: usize, limit: Option<usize>) -> impl Iterator<Item = T> {
    rows.into_iter()
        .skip(offset)
        .take(limit.unwrap_or(usize::MAX))
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ServiceQuery {
    /// Services of this host.
    pub host: Option<String>,
    /// Services of hosts in this host group.
    pub hostgroup: Option<String>,
    /// Services in this service group.
    pub group: Option<String>,
    /// Comma-separated: `ok,warning,critical,unknown,pending`.
    pub state: Option<String>,
    /// Only services that are not OK (and not pending).
    #[serde(default)]
    pub problems: bool,
    /// Only confirmed (hard) states.
    #[serde(default)]
    pub hard: bool,
    /// Only problems that are neither acknowledged nor in a downtime.
    #[serde(default)]
    pub unhandled: bool,
    /// Text in the host, service, title or output, ignoring case.
    pub q: Option<String>,
    /// `host`, `service`, `status`, `last_check`, `duration`, `attempt`;
    /// config order without.
    pub sort: Option<String>,
    #[serde(default)]
    pub desc: bool,
    #[serde(default)]
    pub offset: usize,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
enum ServiceSort {
    Host,
    Service,
    Status,
    LastCheck,
    Duration,
    Attempt,
}

#[derive(Debug, Serialize)]
pub(crate) struct ServiceList {
    /// Per state, of the services that match every filter but `state`.
    pub counts: Counts,
    /// Services that match every filter.
    pub total: usize,
    /// The requested page of them.
    pub services: Vec<ServiceView>,
}

/// Whether a problem is unhandled: neither acknowledged nor in a downtime.
fn unhandled(live: &Live, service: &Service, now: SystemTime) -> bool {
    let status = live.ops.status(&live.config, service, now);
    !status.acknowledged && status.downtime.is_empty()
}

fn is_problem(state: Option<State>) -> bool {
    state.is_some_and(|s| s != State::Ok)
}

pub(crate) fn services(live: &Live, q: &ServiceQuery) -> Result<ServiceList, ApiError> {
    let config = &live.config;
    let now = SystemTime::now();
    let states = q.state.as_deref().map(parse_states).transpose()?;
    let sort = sort_key(
        q.sort.as_deref(),
        &[
            ("host", ServiceSort::Host),
            ("service", ServiceSort::Service),
            ("status", ServiceSort::Status),
            ("last_check", ServiceSort::LastCheck),
            ("duration", ServiceSort::Duration),
            ("attempt", ServiceSort::Attempt),
        ],
    )?;
    let text = q.q.as_deref().map(str::trim).filter(|t| !t.is_empty());
    let text = text.map(str::to_lowercase);

    let candidates: Box<dyn Iterator<Item = &Service>> = match &q.host {
        Some(name) => match config.host_named(name) {
            Some((i, _)) => Box::new(config.services_of(i)),
            None => Box::new(std::iter::empty()),
        },
        None => Box::new(config.services.iter()),
    };
    let mut counts = Counts::default();
    let mut rows: Vec<(&Service, Option<&LiveService>)> = Vec::new();
    for s in candidates {
        let l = live.services.get(&s.id);
        let state = l.map(|l| l.state.state);
        let host = config.host(s);
        let keep = q.hostgroup.as_ref().is_none_or(|g| host.in_group(g))
            && q.group.as_ref().is_none_or(|g| s.in_group(g))
            && (!q.problems || is_problem(state))
            && (!q.hard || l.is_some_and(|l| l.state.state_type == StateType::Hard))
            && text.as_ref().is_none_or(|t| {
                contains(&host.name, t)
                    || contains(&s.name, t)
                    || s.title.as_deref().is_some_and(|x| contains(x, t))
                    || l.is_some_and(|l| contains(&l.output, t))
            })
            && (!q.unhandled || (is_problem(state) && unhandled(live, s, now)));
        if !keep {
            continue;
        }
        counts.add(state);
        if states.as_ref().is_none_or(|list| list.contains(&state)) {
            rows.push((s, l));
        }
    }

    if let Some(key) = sort {
        // One integer key per row, so sorting compares numbers: names by
        // their place in natural order, worked out once.
        let nanos = |l: Option<&LiveService>, f: fn(&LiveService) -> SystemTime| -> i128 {
            l.map_or(0, |l| {
                f(l).duration_since(SystemTime::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos() as i128)
            })
        };
        let hosts = host_ranks(config);
        let names = name_ranks(rows.iter().map(|(s, _)| display(s)));
        let mut keyed: Vec<(i128, usize, usize, usize)> = rows
            .iter()
            .enumerate()
            .map(|(i, (s, l))| {
                let (host, name) = (hosts[s.host], names[display(s)]);
                let primary = match key {
                    ServiceSort::Host => host as i128,
                    ServiceSort::Service => name as i128,
                    ServiceSort::Status => severity(l.map(|l| l.state.state)).into(),
                    ServiceSort::LastCheck => nanos(*l, |l| l.state.last_check),
                    // Longest in its state last: the most recent change first.
                    ServiceSort::Duration => -nanos(*l, |l| l.state.last_change),
                    ServiceSort::Attempt => l.map_or(0, |l| l.state.attempt).into(),
                };
                (if q.desc { -primary } else { primary }, host, name, i)
            })
            .collect();
        // Equal titles on one host fall back to config order.
        keyed.sort_unstable();
        let mut slots: Vec<Option<(&Service, Option<&LiveService>)>> =
            rows.into_iter().map(Some).collect();
        rows = keyed
            .into_iter()
            .filter_map(|(_, _, _, i)| slots[i].take())
            .collect();
    }

    let total = rows.len();
    let services = page(rows, q.offset, q.limit)
        .map(|(s, l)| ServiceView::new(config, s, l, &live.ops.status(config, s, now)))
        .collect();
    Ok(ServiceList {
        counts,
        total,
        services,
    })
}

/// Per host (by position), its place in natural name order.
fn host_ranks(config: &Config) -> Vec<usize> {
    let mut order: Vec<usize> = (0..config.hosts.len()).collect();
    order.sort_by(|&a, &b| natural(&config.hosts[a].name, &config.hosts[b].name));
    let mut rank = vec![0; order.len()];
    for (place, host) in order.into_iter().enumerate() {
        rank[host] = place;
    }
    rank
}

/// Each distinct name's place in natural order.
fn name_ranks<'a>(names: impl Iterator<Item = &'a str>) -> HashMap<&'a str, usize> {
    let mut distinct: Vec<&str> = names.collect();
    distinct.sort_unstable();
    distinct.dedup();
    distinct.sort_by(|a, b| natural(a, b));
    distinct
        .into_iter()
        .enumerate()
        .map(|(i, n)| (n, i))
        .collect()
}

/// Title, else name.
fn display(s: &Service) -> &str {
    s.title.as_deref().unwrap_or(&s.name)
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct HostQuery {
    /// Comma-separated: `up,down,pending`.
    pub state: Option<String>,
    /// Hosts in this host group.
    pub group: Option<String>,
    /// Only DOWN hosts that are neither acknowledged nor in a downtime.
    #[serde(default)]
    pub unhandled: bool,
    /// Text in the name, address or host groups, ignoring case.
    pub q: Option<String>,
    /// `host`, `status`, `address`, `last_check`, `duration`; config order
    /// without.
    pub sort: Option<String>,
    #[serde(default)]
    pub desc: bool,
    #[serde(default)]
    pub offset: usize,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
enum HostSort {
    Host,
    Status,
    Address,
    LastCheck,
    Duration,
}

/// Number of hosts per state.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct HostCounts {
    pub up: usize,
    pub down: usize,
    pub pending: usize,
}

impl HostCounts {
    fn add(&mut self, state: Option<HostState>) {
        match state {
            Some(HostState::Up) => self.up += 1,
            Some(HostState::Down) => self.down += 1,
            None => self.pending += 1,
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct HostList {
    /// Per state, of the hosts that match every filter but `state`.
    pub counts: HostCounts,
    /// Hosts that match every filter.
    pub total: usize,
    pub hosts: Vec<HostView>,
}

/// A host with its host check and the state that gives it.
struct HostRow<'a> {
    index: usize,
    host: &'a Host,
    check: Option<&'a LiveService>,
    state: Option<HostState>,
}

fn host_row<'a>(live: &'a Live, index: usize, host: &'a Host) -> HostRow<'a> {
    let check = live
        .config
        .host_check(host)
        .and_then(|s| live.services.get(&s.id));
    let state = match &host.host_check {
        None => Some(HostState::Up),
        Some(_) => check.map(|l| HostState::from_check(l.state.state)),
    };
    HostRow {
        index,
        host,
        check,
        state,
    }
}

/// Acknowledgement, mute, checks and downtime of a host itself.
pub(crate) fn host_flags(live: &Live, host: &Host, now: SystemTime) -> HostFlags {
    let ops = &live.ops;
    HostFlags {
        acknowledged: ops.flag(FlagKind::Ack, &host.name).is_some(),
        muted: ops.flag(FlagKind::Mute, &host.name).is_some(),
        checks_enabled: ops.checks && ops.flag(FlagKind::NoChecks, &host.name).is_none(),
        downtime: !ops.downtimes_of(&live.config, host, None, now).is_empty(),
    }
}

/// The view of the host at `index`, with the views of its services.
pub(crate) fn host_view(
    live: &Live,
    index: usize,
    now: SystemTime,
) -> (HostView, Vec<ServiceView>) {
    let config: &Config = &live.config;
    let host = &config.hosts[index];
    let services: Vec<ServiceView> = config
        .services_of(index)
        .map(|s| {
            let status = live.ops.status(config, s, now);
            ServiceView::new(config, s, live.services.get(&s.id), &status)
        })
        .collect();
    let refs: Vec<&ServiceView> = services.iter().collect();
    let view = HostView::new(host, &refs, &host_flags(live, host, now));
    (view, services)
}

/// A DOWN host is unhandled unless it, or its host check, is acknowledged
/// or it is in a downtime.
fn host_unhandled(live: &Live, row: &HostRow, now: SystemTime) -> bool {
    if row.state != Some(HostState::Down) {
        return false;
    }
    let flags = host_flags(live, row.host, now);
    let check_ack = live
        .config
        .host_check(row.host)
        .is_some_and(|s| live.ops.status(&live.config, s, now).acknowledged);
    !(flags.acknowledged || check_ack || flags.downtime)
}

pub(crate) fn hosts(live: &Live, q: &HostQuery) -> Result<HostList, ApiError> {
    let config = &live.config;
    let now = SystemTime::now();
    let states = q.state.as_deref().map(parse_host_states).transpose()?;
    let sort = sort_key(
        q.sort.as_deref(),
        &[
            ("host", HostSort::Host),
            ("status", HostSort::Status),
            ("address", HostSort::Address),
            ("last_check", HostSort::LastCheck),
            ("duration", HostSort::Duration),
        ],
    )?;
    let text = q.q.as_deref().map(str::trim).filter(|t| !t.is_empty());
    let text = text.map(str::to_lowercase);

    let mut counts = HostCounts::default();
    let mut rows = Vec::new();
    for (i, host) in config.hosts.iter().enumerate() {
        if q.group.as_ref().is_some_and(|g| !host.in_group(g)) {
            continue;
        }
        if let Some(t) = &text
            && !(contains(&host.name, t)
                || contains(&host.address, t)
                || host.groups.iter().any(|g| contains(&g.name, t)))
        {
            continue;
        }
        let row = host_row(live, i, host);
        if q.unhandled && !host_unhandled(live, &row, now) {
            continue;
        }
        counts.add(row.state);
        if states.as_ref().is_none_or(|list| list.contains(&row.state)) {
            rows.push(row);
        }
    }

    if let Some(key) = sort {
        let time = |r: &HostRow, f: fn(&LiveService) -> SystemTime| {
            r.check.map_or(SystemTime::UNIX_EPOCH, f)
        };
        rows.sort_by(|a, b| {
            let primary = match key {
                HostSort::Host => natural(&a.host.name, &b.host.name),
                HostSort::Status => host_severity(a.state).cmp(&host_severity(b.state)),
                HostSort::Address => natural(&a.host.address, &b.host.address),
                HostSort::LastCheck => {
                    time(a, |l| l.state.last_check).cmp(&time(b, |l| l.state.last_check))
                }
                HostSort::Duration => {
                    time(b, |l| l.state.last_change).cmp(&time(a, |l| l.state.last_change))
                }
            };
            let primary = if q.desc { primary.reverse() } else { primary };
            primary.then_with(|| natural(&a.host.name, &b.host.name))
        });
    }

    let total = rows.len();
    let hosts = page(rows, q.offset, q.limit)
        .map(|r| host_view(live, r.index, now).0)
        .collect();
    Ok(HostList {
        counts,
        total,
        hosts,
    })
}

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct HostSummary {
    pub total: usize,
    #[serde(flatten)]
    pub counts: HostCounts,
    /// DOWN, not acknowledged, not in a downtime.
    pub unhandled: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct ServiceSummary {
    pub total: usize,
    #[serde(flatten)]
    pub counts: Counts,
    /// Not OK, not acknowledged, not in a downtime.
    pub unhandled: usize,
}

/// Status totals of everything, for the overview and the stream.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct Summary {
    /// Changes when a state, a flag or the configuration changes; lists
    /// showing a page refetch it then.
    pub generation: u64,
    pub hosts: HostSummary,
    pub services: ServiceSummary,
}

pub(crate) fn summary(live: &Live, generation: u64) -> Summary {
    let config = &live.config;
    let now = SystemTime::now();
    let mut out = Summary {
        generation,
        ..Summary::default()
    };
    for (i, host) in config.hosts.iter().enumerate() {
        let row = host_row(live, i, host);
        out.hosts.counts.add(row.state);
        if host_unhandled(live, &row, now) {
            out.hosts.unhandled += 1;
        }
    }
    out.hosts.total = config.hosts.len();
    for s in &config.services {
        let state = live.services.get(&s.id).map(|l| l.state.state);
        out.services.counts.add(state);
        if is_problem(state) && unhandled(live, s, now) {
            out.services.unhandled += 1;
        }
    }
    out.services.total = config.services.len();
    out
}

/// Status totals of one host or service group.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct GroupCounts {
    /// Host groups only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hosts: Option<HostCounts>,
    pub services: Counts,
}

/// Totals per host group and per service group, by name.
pub(crate) fn group_counts(
    live: &Live,
) -> (HashMap<String, GroupCounts>, HashMap<String, GroupCounts>) {
    let config = &live.config;
    let mut hostgroups: HashMap<String, GroupCounts> = config
        .hostgroups
        .iter()
        .map(|g| {
            let counts = GroupCounts {
                hosts: Some(HostCounts::default()),
                ..GroupCounts::default()
            };
            (g.name.clone(), counts)
        })
        .collect();
    let mut servicegroups: HashMap<String, GroupCounts> = config
        .servicegroups
        .iter()
        .map(|g| (g.name.clone(), GroupCounts::default()))
        .collect();
    for (i, host) in config.hosts.iter().enumerate() {
        let state = host_row(live, i, host).state;
        for g in &host.groups {
            if let Some(c) = hostgroups.get_mut(&g.name).and_then(|c| c.hosts.as_mut()) {
                c.add(state);
            }
        }
    }
    for s in &config.services {
        let state = live.services.get(&s.id).map(|l| l.state.state);
        for g in &config.host(s).groups {
            if let Some(c) = hostgroups.get_mut(&g.name) {
                c.services.add(state);
            }
        }
        for g in &s.groups {
            if let Some(c) = servicegroups.get_mut(&g.name) {
                c.services.add(state);
            }
        }
    }
    (hostgroups, servicegroups)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_naturally() {
        let mut names = vec!["web10", "Web2", "web02a", "db1", "web1", "web", "web2"];
        names.sort_by(|a, b| natural(a, b));
        assert_eq!(
            names,
            ["db1", "web", "web1", "Web2", "web2", "web02a", "web10"]
        );
        assert_eq!(natural("a", "a"), Ordering::Equal);
        assert_eq!(natural("disk_/", "disk_/var"), Ordering::Less);
    }

    #[test]
    fn parses_states() {
        assert_eq!(
            parse_states("ok, pending").unwrap(),
            [Some(State::Ok), None]
        );
        assert!(parse_states("bad").is_err());
        assert_eq!(parse_host_states("down").unwrap(), [Some(HostState::Down)]);
        assert!(parse_host_states("ok").is_err());
    }
}
