//! Runtime operations: acknowledgements, muted notifications, disabled
//! checks, downtimes and the global switches, plus the actions API that
//! changes them. Kept in memory and, with a database, stored there.
//!
//! Targets are a host (`web01`) or a service (`web01/http`). A host flag
//! covers all of its services; a host acknowledgement covers its host
//! check. Downtimes also target host, service and downtime groups, and
//! downtime groups bring recurring windows from the config.

use crate::api::{ApiError, parse_time};
use crate::auth::CurrentUser;
use crate::{Hub, Update};
use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::oneshot;
use tracing::{info, warn};
use watcher_config::{Config, Host, Service};
use watcher_core::State as CheckState;
use watcher_exec::scheduler::{Control, Scheduled};
use watcher_store::{Downtime, Flag, FlagKind};

/// Runtime state that isn't in the config files.
pub(crate) struct Ops {
    pub flags: BTreeMap<(FlagKind, String), Flag>,
    /// One-off downtimes that haven't ended.
    pub downtimes: Vec<Downtime>,
    /// Global switches.
    pub notifications: bool,
    pub checks: bool,
    tz: TimeZone,
}

impl Default for Ops {
    fn default() -> Self {
        Ops {
            flags: BTreeMap::new(),
            downtimes: Vec::new(),
            notifications: true,
            checks: true,
            tz: TimeZone::system(),
        }
    }
}

/// `hostgroup linux: patch day`, or without a comment `hostgroup linux`.
fn label(what: &str, comment: &str) -> String {
    match comment.is_empty() {
        true => what.to_string(),
        false => format!("{what}: {comment}"),
    }
}

/// What is in a downtime, by host or `host/service`: (host, service, which
/// downtime, state).
pub(crate) type InDowntime = BTreeMap<String, (String, Option<String>, String, CheckState)>;

/// A downtime that started or ended for a host (all its services) or one
/// service.
#[derive(Debug, Clone, PartialEq)]
pub struct DowntimeChange {
    pub host: String,
    pub service: Option<String>,
    pub started: bool,
    /// The state then (for a host: of its host check; OK without one).
    pub state: CheckState,
    /// Which downtime, e.g. `hostgroup linux: patch day`.
    pub what: String,
}

/// What applies to one service right now.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Status {
    /// Its problem is acknowledged (for a host check: the host's).
    pub acknowledged: bool,
    /// Its notifications are off (service or host muted).
    pub muted: bool,
    /// Active checks run (not turned off for it, its host or globally).
    pub checks: bool,
    /// The downtimes it is in, e.g. `downtimegroup patchday`.
    pub downtime: Vec<String>,
}

impl Ops {
    pub fn flag(&self, kind: FlagKind, target: &str) -> Option<&Flag> {
        self.flags.get(&(kind, target.to_string()))
    }

    fn has(&self, kind: FlagKind, target: &str) -> bool {
        self.flag(kind, target).is_some()
    }

    pub fn status(&self, config: &Config, service: &Service, now: SystemTime) -> Status {
        let host = config.host(service);
        let ack = match config.is_host_check(service) {
            true => self.has(FlagKind::Ack, &host.name) || self.has(FlagKind::Ack, &service.id),
            false => self.has(FlagKind::Ack, &service.id),
        };
        Status {
            acknowledged: ack,
            muted: self.has(FlagKind::Mute, &host.name) || self.has(FlagKind::Mute, &service.id),
            checks: self.checks
                && !self.has(FlagKind::NoChecks, &host.name)
                && !self.has(FlagKind::NoChecks, &service.id),
            downtime: self.downtimes_of(config, host, Some(service), now),
        }
    }

    /// The downtimes `host` (or `service` on it) is in at `now`.
    pub fn downtimes_of(
        &self,
        config: &Config,
        host: &Host,
        service: Option<&Service>,
        now: SystemTime,
    ) -> Vec<String> {
        let mut out = Vec::new();
        for d in &self.downtimes {
            if d.starts > now || d.ends <= now {
                continue;
            }
            let hit = match d.kind.as_str() {
                "host" => d.target == host.name,
                "service" => service.is_some_and(|s| s.id == d.target),
                "hostgroup" => host.in_group(&d.target),
                "servicegroup" => service.is_some_and(|s| s.in_group(&d.target)),
                "downtimegroup" => config
                    .downtimegroups
                    .iter()
                    .any(|g| g.name == d.target && g.has_host(host)),
                _ => false,
            };
            if hit {
                out.push(label(&format!("{} {}", d.kind, d.target), &d.comment));
            }
        }
        for g in &config.downtimegroups {
            if g.schedule.iter().any(|w| w.active(now, &self.tz).is_some()) && g.has_host(host) {
                let title = g.title.as_deref().unwrap_or_default();
                out.push(label(&format!("downtimegroup {}", g.name), title));
            }
        }
        out.dedup();
        out
    }

    /// Uses `tz` for downtime group windows.
    pub fn set_timezone(&mut self, tz: TimeZone) {
        self.tz = tz;
    }

    /// Jobs whose checks are off, for the scheduler.
    fn paused(&self, config: &Config) -> Control {
        let ids = config
            .services
            .iter()
            .filter(|s| {
                self.has(FlagKind::NoChecks, &s.id)
                    || self.has(FlagKind::NoChecks, &config.host(s).name)
            })
            .map(|s| s.id.clone())
            .collect();
        Control::Pause {
            all: !self.checks,
            ids,
        }
    }
}

impl Hub {
    /// Downtimes that started or ended since the last call, recorded in
    /// the alert history. The first call only takes note of what is in a
    /// downtime. Hosts in a downtime count once, not per service.
    pub async fn downtime_changes(&self) -> Vec<DowntimeChange> {
        let (now_in, changes) = {
            let live = self.read();
            let (config, ops) = (&live.config, &live.ops);
            let now = SystemTime::now();
            let mut now_in = InDowntime::new();
            for host in &config.hosts {
                if let Some(what) = ops.downtimes_of(config, host, None, now).into_iter().next() {
                    let state = config
                        .host_check(host)
                        .and_then(|s| live.services.get(&s.id))
                        .map_or(CheckState::Ok, |l| l.state.state);
                    now_in.insert(host.name.clone(), (host.name.clone(), None, what, state));
                }
            }
            for s in &config.services {
                let host = config.host(s);
                if now_in.contains_key(&host.name) {
                    continue;
                }
                if let Some(what) = ops
                    .downtimes_of(config, host, Some(s), now)
                    .into_iter()
                    .next()
                {
                    let state = live
                        .services
                        .get(&s.id)
                        .map_or(CheckState::Ok, |l| l.state.state);
                    now_in.insert(
                        s.id.clone(),
                        (host.name.clone(), Some(s.name.clone()), what, state),
                    );
                }
            }
            let seen = self.downtime_seen.lock().unwrap_or_else(|e| e.into_inner());
            let changes: Vec<DowntimeChange> = match &*seen {
                None => Vec::new(),
                Some(before) => {
                    let started = now_in.iter().filter(|(k, _)| !before.contains_key(*k)).map(
                        |(_, (host, service, what, state))| DowntimeChange {
                            host: host.clone(),
                            service: service.clone(),
                            started: true,
                            state: *state,
                            what: what.clone(),
                        },
                    );
                    let ended = before.iter().filter(|(k, _)| !now_in.contains_key(*k)).map(
                        |(_, (host, service, what, _))| {
                            let id = match service {
                                Some(s) => format!("{host}/{s}"),
                                None => host.clone(),
                            };
                            let state = match service {
                                Some(_) => live.services.get(&id).map(|l| l.state.state),
                                None => config
                                    .hosts
                                    .iter()
                                    .find(|h| h.name == *host)
                                    .and_then(|h| config.host_check(h))
                                    .and_then(|s| live.services.get(&s.id))
                                    .map(|l| l.state.state),
                            };
                            DowntimeChange {
                                host: host.clone(),
                                service: service.clone(),
                                started: false,
                                state: state.unwrap_or(CheckState::Ok),
                                what: what.clone(),
                            }
                        },
                    );
                    started.chain(ended).collect()
                }
            };
            (now_in, changes)
        };
        *self.downtime_seen.lock().unwrap_or_else(|e| e.into_inner()) = Some(now_in);
        for c in &changes {
            let kind = if c.started {
                "downtime_start"
            } else {
                "downtime_end"
            };
            let state = match c.service {
                Some(_) => c.state.as_str().to_lowercase(),
                None => host_state(c.state),
            };
            self.history(&c.host, c.service.as_deref(), kind, state, c.what.clone())
                .await;
        }
        changes
    }

    /// Adds an acknowledgement or downtime to the alert history.
    async fn history(
        &self,
        host: &str,
        service: Option<&str>,
        kind: &'static str,
        state: String,
        output: String,
    ) {
        let Some(store) = &self.store else { return };
        let event = watcher_store::OpsEvent {
            host: host.into(),
            service: service.map(String::from),
            kind,
            state,
            output,
            time: SystemTime::now(),
        };
        if let Err(err) = store.add_event(&event).await {
            warn!("cannot record {kind} of {host}: {err}");
        }
    }

    /// Loads flags, downtimes and switches from the database, and tells the
    /// scheduler which checks are off.
    pub async fn load_ops(&self) -> Result<(), watcher_store::Error> {
        if let Some(store) = &self.store {
            let flags = store.flags().await?;
            let downtimes = store.downtimes().await?;
            let switch = |v: Option<serde_json::Value>| v.and_then(|v| v.as_bool()).unwrap_or(true);
            let notifications = switch(store.setting("notifications").await?);
            let checks = switch(store.setting("checks").await?);
            let mut live = self.write();
            live.ops.flags = flags
                .into_iter()
                .map(|f| ((f.kind, f.target.clone()), f))
                .collect();
            live.ops.downtimes = downtimes;
            live.ops.notifications = notifications;
            live.ops.checks = checks;
        }
        self.sync_scheduler();
        Ok(())
    }

    /// Sends the checks that are off to the scheduler.
    pub(crate) fn sync_scheduler(&self) {
        let pause = {
            let live = self.read();
            live.ops.paused(&live.config)
        };
        if self.control.try_send(pause).is_err() {
            warn!("scheduler queue full: check pausing not updated");
        }
    }

    /// Why notifications for this result are held back, if they are. A
    /// recovery ends an acknowledgement and is still sent.
    pub fn suppressed(&self, id: &str, state: CheckState) -> Option<String> {
        if self.host_down(id) {
            return Some("host is DOWN".into());
        }
        let live = self.read();
        let service = live.config.service(id)?;
        let ops = &live.ops;
        if !ops.notifications {
            return Some("notifications are off".into());
        }
        let status = ops.status(&live.config, service, SystemTime::now());
        if status.muted {
            return Some("muted".into());
        }
        if let Some(d) = status.downtime.first() {
            return Some(format!("in downtime ({d})"));
        }
        if status.acknowledged && state != CheckState::Ok {
            return Some("acknowledged".into());
        }
        None
    }

    /// Why a downtime notification for a host or service is held back:
    /// notifications off, or muted.
    pub fn downtime_suppressed(&self, host: &str, service: Option<&str>) -> Option<String> {
        let live = self.read();
        let ops = &live.ops;
        if !ops.notifications {
            return Some("notifications are off".into());
        }
        let id = service.map(|s| format!("{host}/{s}"));
        if ops.has(FlagKind::Mute, host) || id.is_some_and(|id| ops.has(FlagKind::Mute, &id)) {
            return Some("muted".into());
        }
        None
    }

    /// Drops acknowledgements of `id` (and of its host, for a host check)
    /// once it is OK again.
    pub(crate) fn clear_acks(&self, id: &str) {
        let targets: Vec<String> = {
            let live = self.read();
            let Some(service) = live.config.service(id) else {
                return;
            };
            let mut t = vec![service.id.clone()];
            if live.config.is_host_check(service) {
                t.push(live.config.host(service).name.clone());
            }
            t.into_iter()
                .filter(|t| live.ops.has(FlagKind::Ack, t))
                .collect()
        };
        if targets.is_empty() {
            return;
        }
        {
            let mut live = self.write();
            for t in &targets {
                live.ops.flags.remove(&(FlagKind::Ack, t.clone()));
            }
        }
        info!("acknowledgement of {} ended: OK", targets.join(", "));
        if let Some(store) = self.store.clone() {
            tokio::spawn(async move {
                for t in targets {
                    if let Err(err) = store.clear_flag(FlagKind::Ack, &t).await {
                        warn!("cannot remove acknowledgement of {t}: {err}");
                    }
                }
            });
        }
    }
}

type Shared = State<Arc<Hub>>;
type User = Option<Extension<CurrentUser>>;

/// Who acts: the user, or `local user` without login.
fn author(hub: &Hub, user: &User) -> Result<String, ApiError> {
    match user {
        Some(Extension(CurrentUser(u))) => Ok(u.username.clone()),
        None if !hub.auth.required => Ok("local user".into()),
        None => Err(ApiError::Unauthorized),
    }
}

fn require_admin(hub: &Hub, user: &User) -> Result<String, ApiError> {
    match user {
        Some(Extension(CurrentUser(u))) if u.admin => Ok(u.username.clone()),
        None if !hub.auth.required => Ok("local user".into()),
        _ => Err(ApiError::Forbidden("admin rights required".into())),
    }
}

/// What an action applies to.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActionBody {
    #[serde(default)]
    hosts: Vec<String>,
    /// `host/service` ids.
    #[serde(default)]
    services: Vec<String>,
    #[serde(default)]
    hostgroups: Vec<String>,
    #[serde(default)]
    servicegroups: Vec<String>,
    #[serde(default)]
    downtimegroups: Vec<String>,
    #[serde(default)]
    comment: String,
    /// Downtimes: start (default now) and end, or a duration (`2h`).
    start: Option<String>,
    end: Option<String>,
    duration: Option<String>,
}

/// Hosts and services an action reaches, groups expanded.
#[derive(Debug, Default)]
struct Reach {
    hosts: BTreeSet<String>,
    services: BTreeSet<String>,
}

fn resolve(config: &Config, body: &ActionBody) -> Result<Reach, ApiError> {
    let mut reach = Reach::default();
    let mut unknown = Vec::new();
    for h in &body.hosts {
        match config.hosts.iter().any(|x| x.name == *h) {
            true => _ = reach.hosts.insert(h.clone()),
            false => unknown.push(format!("host {h:?}")),
        }
    }
    for s in &body.services {
        match config.service(s) {
            Some(_) => _ = reach.services.insert(s.clone()),
            None => unknown.push(format!("service {s:?}")),
        }
    }
    for g in &body.hostgroups {
        if !config.hostgroups.iter().any(|x| x.name == *g) {
            unknown.push(format!("hostgroup {g:?}"));
            continue;
        }
        reach.hosts.extend(
            config
                .hosts
                .iter()
                .filter(|h| h.in_group(g))
                .map(|h| h.name.clone()),
        );
    }
    for g in &body.servicegroups {
        if !config.servicegroups.iter().any(|x| x.name == *g) {
            unknown.push(format!("servicegroup {g:?}"));
            continue;
        }
        reach.services.extend(
            config
                .services
                .iter()
                .filter(|s| s.in_group(g))
                .map(|s| s.id.clone()),
        );
    }
    for g in &body.downtimegroups {
        let Some(group) = config.downtimegroups.iter().find(|x| x.name == *g) else {
            unknown.push(format!("downtimegroup {g:?}"));
            continue;
        };
        reach.hosts.extend(
            config
                .hosts
                .iter()
                .filter(|h| group.has_host(h))
                .map(|h| h.name.clone()),
        );
    }
    if !unknown.is_empty() {
        return Err(ApiError::NotFound(format!(
            "unknown {}",
            unknown.join(", ")
        )));
    }
    if reach.hosts.is_empty() && reach.services.is_empty() {
        return Err(ApiError::BadRequest(
            "no targets: give hosts, services, hostgroups, servicegroups or downtimegroups".into(),
        ));
    }
    Ok(reach)
}

/// The services of `reach`: listed ones and all of each host.
fn all_services(config: &Config, reach: &Reach) -> BTreeSet<String> {
    let mut out = reach.services.clone();
    out.extend(
        config
            .services
            .iter()
            .filter(|s| reach.hosts.contains(&config.host(s).name))
            .map(|s| s.id.clone()),
    );
    out
}

#[derive(Debug, Serialize)]
pub(crate) struct ActionResult {
    action: String,
    /// Hosts and services it was applied to.
    done: Vec<String>,
    /// Targets left out, with why (e.g. acknowledging an OK service).
    skipped: Vec<Skipped>,
    /// Created downtimes.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    downtimes: Vec<Downtime>,
}

#[derive(Debug, Serialize)]
struct Skipped {
    target: String,
    reason: String,
}

/// `POST /actions/{action}`: recheck, ack, unack, mute, unmute,
/// disable-checks, enable-checks, downtime.
pub(crate) async fn action(
    State(hub): Shared,
    user: User,
    Path(action): Path<String>,
    Json(body): Json<ActionBody>,
) -> Result<Json<ActionResult>, ApiError> {
    let who = author(&hub, &user)?;
    let config = hub.config();
    let reach = resolve(&config, &body)?;
    let mut result = ActionResult {
        action: action.clone(),
        done: Vec::new(),
        skipped: Vec::new(),
        downtimes: Vec::new(),
    };
    match action.as_str() {
        "recheck" => {
            let ids: Vec<String> = all_services(&config, &reach)
                .into_iter()
                .filter(|id| {
                    let passive = config.service(id).is_some_and(|s| s.settings.passive);
                    if passive {
                        result.skipped.push(Skipped {
                            target: id.clone(),
                            reason: "passive".into(),
                        });
                    }
                    !passive
                })
                .collect();
            hub.control
                .send(Control::RunNow(ids.clone()))
                .await
                .map_err(|_| ApiError::Unavailable("the scheduler is not running".into()))?;
            result.done = ids;
        }
        "ack" => {
            let mut targets = Vec::new();
            {
                let live = hub.read();
                let state = |id: &str| live.services.get(id).map(|l| l.state.state);
                for h in &reach.hosts {
                    let host = config.hosts.iter().find(|x| x.name == *h);
                    match host.and_then(|h| config.host_check(h)) {
                        None => result.skipped.push(Skipped {
                            target: h.clone(),
                            reason: "no host check".into(),
                        }),
                        Some(hc) if matches!(state(&hc.id), None | Some(CheckState::Ok)) => {
                            result.skipped.push(Skipped {
                                target: h.clone(),
                                reason: "UP".into(),
                            })
                        }
                        Some(_) => targets.push(h.clone()),
                    }
                }
                for s in &reach.services {
                    match state(s) {
                        None | Some(CheckState::Ok) => result.skipped.push(Skipped {
                            target: s.clone(),
                            reason: "no problem".into(),
                        }),
                        Some(_) => targets.push(s.clone()),
                    }
                }
            }
            result.done = set_flags(&hub, FlagKind::Ack, targets, &who, &body.comment).await?;
            for target in &result.done {
                let (host, service, state) = {
                    let live = hub.read();
                    match target.split_once('/') {
                        Some((h, s)) => {
                            let state = live.services.get(target).map(|l| l.state.state);
                            (
                                h.to_string(),
                                Some(s.to_string()),
                                state.map(|s| s.as_str().to_lowercase()),
                            )
                        }
                        None => {
                            let state = config
                                .hosts
                                .iter()
                                .find(|h| h.name == *target)
                                .and_then(|h| config.host_check(h))
                                .and_then(|s| live.services.get(&s.id))
                                .map(|l| host_state(l.state.state));
                            (target.clone(), None, state)
                        }
                    }
                };
                let output = label(&who, &body.comment);
                let state = state.unwrap_or_else(|| "pending".into());
                hub.history(&host, service.as_deref(), "ack", state, output)
                    .await;
            }
        }
        "mute" | "disable-checks" => {
            let kind = match action.as_str() {
                "mute" => FlagKind::Mute,
                _ => FlagKind::NoChecks,
            };
            let targets = reach.hosts.iter().chain(&reach.services).cloned().collect();
            result.done = set_flags(&hub, kind, targets, &who, &body.comment).await?;
        }
        "unack" | "unmute" | "enable-checks" => {
            let kind = match action.as_str() {
                "unack" => FlagKind::Ack,
                "unmute" => FlagKind::Mute,
                _ => FlagKind::NoChecks,
            };
            // A host's services too: unmuting a host unmutes everything on it.
            let mut targets: BTreeSet<String> = reach.hosts.clone();
            targets.extend(all_services(&config, &reach));
            result.done = clear_flags(&hub, kind, targets.into_iter().collect()).await?;
        }
        "downtime" => {
            let now = SystemTime::now();
            let starts = match &body.start {
                Some(s) => parse_moment(s, now)?,
                None => now,
            };
            let ends = match (&body.end, &body.duration) {
                (Some(e), None) => parse_moment(e, now)?,
                (None, Some(d)) => {
                    starts
                        + humantime::parse_duration(d).map_err(|e| {
                            ApiError::BadRequest(format!("invalid duration {d:?}: {e}"))
                        })?
                }
                _ => return Err(ApiError::BadRequest("give end or duration".into())),
            };
            if ends <= starts {
                return Err(ApiError::BadRequest("end must be after start".into()));
            }
            // Groups stay groups: a host joining later is in the downtime too.
            let targets = [
                ("host", &body.hosts),
                ("service", &body.services),
                ("hostgroup", &body.hostgroups),
                ("servicegroup", &body.servicegroups),
                ("downtimegroup", &body.downtimegroups),
            ];
            for (kind, names) in targets {
                for target in names {
                    let d = Downtime {
                        id: 0,
                        kind: kind.into(),
                        target: target.clone(),
                        starts,
                        ends,
                        author: who.clone(),
                        comment: body.comment.clone(),
                        created: now,
                    };
                    let d = match &hub.store {
                        Some(store) => store.add_downtime(&d).await?,
                        None => Downtime {
                            id: hub.next_id(),
                            ..d
                        },
                    };
                    result.done.push(format!("{kind} {target}"));
                    result.downtimes.push(d.clone());
                    hub.write().ops.downtimes.push(d);
                }
            }
            info!("{who}: downtime for {}", result.done.join(", "));
        }
        other => {
            return Err(ApiError::NotFound(format!(
                "unknown action {other:?} (recheck, ack, unack, mute, unmute, disable-checks, \
                 enable-checks, downtime)"
            )));
        }
    }
    if action != "recheck" {
        hub.ops_changed();
    }
    Ok(Json(result))
}

/// `up` or `down` for a host check state.
fn host_state(state: CheckState) -> String {
    watcher_core::HostState::from_check(state)
        .as_str()
        .to_lowercase()
}

/// RFC 3339, unix seconds or `now` (not a duration back: downtimes look
/// ahead).
fn parse_moment(s: &str, now: SystemTime) -> Result<SystemTime, ApiError> {
    if humantime::parse_duration(s.trim()).is_ok() {
        return Err(ApiError::BadRequest(format!(
            "invalid time {s:?}: use RFC 3339 or unix seconds; for a length use duration"
        )));
    }
    parse_time(s, now)
}

async fn set_flags(
    hub: &Hub,
    kind: FlagKind,
    targets: Vec<String>,
    author: &str,
    comment: &str,
) -> Result<Vec<String>, ApiError> {
    let now = SystemTime::now();
    for target in &targets {
        let flag = Flag {
            kind,
            target: target.clone(),
            author: author.into(),
            comment: comment.into(),
            created: now,
        };
        if let Some(store) = &hub.store {
            store.set_flag(&flag).await?;
        }
        hub.write().ops.flags.insert((kind, target.clone()), flag);
    }
    if !targets.is_empty() {
        info!("{author}: {} {}", kind.as_str(), targets.join(", "));
    }
    Ok(targets)
}

/// Clears the flags of `targets` that are set; returns those.
async fn clear_flags(
    hub: &Hub,
    kind: FlagKind,
    targets: Vec<String>,
) -> Result<Vec<String>, ApiError> {
    let set: Vec<String> = {
        let live = hub.read();
        targets
            .into_iter()
            .filter(|t| live.ops.has(kind, t))
            .collect()
    };
    for target in &set {
        if let Some(store) = &hub.store {
            store.clear_flag(kind, target).await?;
        }
        hub.write().ops.flags.remove(&(kind, target.clone()));
    }
    Ok(set)
}

impl Hub {
    /// After a change of flags, downtimes or switches: tell the scheduler
    /// and the clients.
    pub(crate) fn ops_changed(&self) {
        self.sync_scheduler();
        let _ = self.updates.send(Update::Reload);
    }

    /// Ids for downtimes without a database.
    fn next_id(&self) -> i64 {
        let live = self.read();
        live.ops.downtimes.iter().map(|d| d.id).max().unwrap_or(0) + 1
    }
}

/// `GET /flags`: acknowledgements, muted and disabled checks.
pub(crate) async fn flags(State(hub): Shared) -> Json<Vec<Flag>> {
    Json(hub.read().ops.flags.values().cloned().collect())
}

/// A downtime as listed: one-off (with `id`) or from a downtime group's
/// schedule (with `window`).
#[derive(Debug, Serialize)]
pub(crate) struct DowntimeView {
    id: Option<i64>,
    kind: String,
    target: String,
    #[serde(serialize_with = "rfc3339")]
    start: SystemTime,
    #[serde(serialize_with = "rfc3339")]
    end: SystemTime,
    author: Option<String>,
    comment: String,
    /// The schedule entry, e.g. `sun 02:00-04:00`.
    window: Option<String>,
    active: bool,
    /// How many hosts and services it covers now.
    hosts: usize,
    services: usize,
}

fn rfc3339<S: serde::Serializer>(t: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
    s.collect_str(&humantime::format_rfc3339_seconds(*t))
}

/// `GET /downtimes`: one-off downtimes that haven't ended, and the current
/// or next window of every downtime group.
pub(crate) async fn downtimes(State(hub): Shared) -> Json<Vec<DowntimeView>> {
    let live = hub.read();
    let (config, ops) = (&live.config, &live.ops);
    let now = SystemTime::now();
    let count = |kind: &str, target: &str| -> (usize, usize) {
        let host_hit = |h: &Host| match kind {
            "host" => h.name == target,
            "hostgroup" => h.in_group(target),
            "downtimegroup" => config
                .downtimegroups
                .iter()
                .any(|g| g.name == target && g.has_host(h)),
            _ => false,
        };
        let hosts = config.hosts.iter().filter(|h| host_hit(h)).count();
        let services = config
            .services
            .iter()
            .filter(|s| {
                host_hit(config.host(s))
                    || (kind == "service" && s.id == target)
                    || (kind == "servicegroup" && s.in_group(target))
            })
            .count();
        (hosts, services)
    };
    let mut out: Vec<DowntimeView> = ops
        .downtimes
        .iter()
        .filter(|d| d.ends > now)
        .map(|d| {
            let (hosts, services) = count(&d.kind, &d.target);
            DowntimeView {
                id: Some(d.id),
                kind: d.kind.clone(),
                target: d.target.clone(),
                start: d.starts,
                end: d.ends,
                author: Some(d.author.clone()),
                comment: d.comment.clone(),
                window: None,
                active: d.starts <= now,
                hosts,
                services,
            }
        })
        .collect();
    for g in &config.downtimegroups {
        let (hosts, services) = count("downtimegroup", &g.name);
        for w in &g.schedule {
            let (window, active) = match w.active(now, &ops.tz) {
                Some(span) => (Some(span), true),
                None => (w.next(now, &ops.tz), false),
            };
            let Some((start, end)) = window else {
                continue;
            };
            out.push(DowntimeView {
                id: None,
                kind: "downtimegroup".into(),
                target: g.name.clone(),
                start,
                end,
                author: None,
                comment: g.title.clone().unwrap_or_default(),
                window: Some(w.to_string()),
                active,
                hosts,
                services,
            });
        }
    }
    out.sort_by_key(|d| (!d.active, d.start));
    Json(out)
}

/// `DELETE /downtimes/{id}`: ends a one-off downtime.
pub(crate) async fn cancel_downtime(
    State(hub): Shared,
    user: User,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let who = author(&hub, &user)?;
    let known = hub.read().ops.downtimes.iter().any(|d| d.id == id);
    if !known {
        return Err(ApiError::NotFound(format!("no downtime {id}")));
    }
    if let Some(store) = &hub.store {
        store.cancel_downtime(id).await?;
    }
    hub.write().ops.downtimes.retain(|d| d.id != id);
    info!("{who}: downtime {id} cancelled");
    hub.ops_changed();
    Ok(Json(json!({ "cancelled": id })))
}

/// `GET /process`: what the process is and does.
pub(crate) async fn process(State(hub): Shared) -> Result<Json<serde_json::Value>, ApiError> {
    let schedule = schedule(&hub).await?;
    let (running, paused, passive) = schedule.iter().fold((0, 0, 0), |(r, p, s), j| {
        (
            r + usize::from(j.running),
            p + usize::from(j.paused),
            s + usize::from(j.passive),
        )
    });
    let live = hub.read();
    let config = &live.config;
    let ops = &live.ops;
    let now = SystemTime::now();
    let in_downtime = config
        .services
        .iter()
        .filter(|s| {
            !ops.downtimes_of(config, config.host(s), Some(s), now)
                .is_empty()
        })
        .count();
    let count = |kind: FlagKind| ops.flags.keys().filter(|(k, _)| *k == kind).count();
    let results = hub.results.lock().unwrap_or_else(|e| e.into_inner());
    Ok(Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "pid": std::process::id(),
        "instance": hub.instance.get(),
        "started": humantime::format_rfc3339_seconds(hub.started).to_string(),
        "uptime_s": hub.started.elapsed().unwrap_or_default().as_secs(),
        "config_loaded": humantime::format_rfc3339_seconds(live.loaded).to_string(),
        "database": hub.store.as_ref().map(|s| s.backend().to_string()),
        "notifications": ops.notifications,
        "checks": ops.checks,
        "hosts": config.hosts.len(),
        "services": config.services.len(),
        "scheduler": {
            "jobs": schedule.len(),
            "running": running,
            "paused": paused,
            "passive": passive,
        },
        "results": {
            "last_1m": results.since(now, Duration::from_secs(60)),
            "last_5m": results.since(now, Duration::from_secs(300)),
            "last_15m": results.since(now, Duration::from_secs(900)),
        },
        "acknowledged": count(FlagKind::Ack),
        "muted": count(FlagKind::Mute),
        "checks_off": count(FlagKind::NoChecks),
        "in_downtime": in_downtime,
    })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProcessBody {
    notifications: Option<bool>,
    checks: Option<bool>,
}

/// `PUT /process`: the global switches. Admins only.
pub(crate) async fn set_process(
    State(hub): Shared,
    user: User,
    Json(body): Json<ProcessBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let who = require_admin(&hub, &user)?;
    for (name, value) in [
        ("notifications", body.notifications),
        ("checks", body.checks),
    ] {
        let Some(on) = value else { continue };
        if let Some(store) = &hub.store {
            store.set_setting(name, &json!(on)).await?;
        }
        let mut live = hub.write();
        match name {
            "notifications" => live.ops.notifications = on,
            _ => live.ops.checks = on,
        }
        info!("{who}: {name} {}", if on { "on" } else { "off" });
    }
    hub.ops_changed();
    let live = hub.read();
    Ok(Json(json!({
        "notifications": live.ops.notifications,
        "checks": live.ops.checks,
    })))
}

async fn schedule(hub: &Hub) -> Result<Vec<Scheduled>, ApiError> {
    let (reply, rx) = oneshot::channel();
    let unavailable = || ApiError::Unavailable("the scheduler is not running".into());
    hub.control
        .send(Control::Status(reply))
        .await
        .map_err(|_| unavailable())?;
    rx.await.map_err(|_| unavailable())
}

#[derive(Debug, Deserialize)]
pub(crate) struct ScheduleQuery {
    limit: Option<usize>,
}

/// `GET /scheduler`: services by when they run next.
pub(crate) async fn scheduler(
    State(hub): Shared,
    Query(q): Query<ScheduleQuery>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let list = schedule(&hub).await?;
    let live = hub.read();
    let limit = q.limit.unwrap_or(500);
    let out = list
        .into_iter()
        .filter_map(|j| {
            let service = live.config.service(&j.id)?;
            let last = live
                .services
                .get(&j.id)
                .map(|l| humantime::format_rfc3339_seconds(l.state.last_check).to_string());
            Some(json!({
                "host": live.config.host(service).name,
                "service": service.name,
                "next_check": j.next.map(|t| humantime::format_rfc3339_seconds(t).to_string()),
                "last_check": last,
                "running": j.running,
                "passive": j.passive,
                "paused": j.paused,
                "interval_s": service.settings.interval.as_secs_f64(),
            }))
        })
        .take(limit)
        .collect();
    Ok(Json(out))
}

/// The result of a run that isn't recorded.
#[derive(Debug, Serialize)]
pub(crate) struct RunResult {
    command: String,
    #[serde(flatten)]
    result: watcher_core::CheckResult,
    stdout: String,
    stderr: String,
    /// How the process ended, e.g. `exit 2` or `timeout`.
    termination: String,
    duration_ms: f64,
}

async fn run_once(
    hub: &Hub,
    command: &watcher_exec::CommandLine,
    remote: Option<&watcher_exec::ssh::Remote>,
    timeout: Duration,
    format: watcher_core::Format,
    thresholds: &BTreeMap<String, watcher_core::ThresholdSpec>,
) -> Result<RunResult, ApiError> {
    let path = hub
        .config_files
        .get()
        .map(|f| Arc::from(f.check_dirs.search_path().as_os_str()));
    let options = watcher_exec::ExecOptions {
        timeout,
        output_limit: watcher_exec::DEFAULT_OUTPUT_LIMIT,
        path,
    };
    let execution = match (remote, hub.ssh.get()) {
        (Some(remote), Some(ssh)) => {
            watcher_exec::ssh::run(&ssh.pool, remote, command, &options).await
        }
        (Some(_), None) => {
            return Err(ApiError::Unavailable("SSH is not set up".into()));
        }
        (None, _) => watcher_exec::run(command, &options).await,
    };
    let result = watcher_core::evaluate(&execution.output, format, thresholds);
    let termination = execution.output.termination.to_string();
    let command = match remote {
        Some(r) => format!("{command} (on {})", r.target),
        None => command.to_string(),
    };
    Ok(RunResult {
        command,
        result,
        stdout: execution.output.stdout,
        stderr: execution.output.stderr,
        termination,
        duration_ms: execution.duration.as_secs_f64() * 1000.0,
    })
}

/// `POST /services/{host}/{service}/run`: runs the service's command now
/// and returns the result without recording it (a test run).
pub(crate) async fn run_service(
    State(hub): Shared,
    user: User,
    Path((host, name)): Path<(String, String)>,
) -> Result<Json<RunResult>, ApiError> {
    author(&hub, &user)?;
    let id = format!("{host}/{name}");
    let service = hub
        .config()
        .service(&id)
        .cloned()
        .ok_or_else(|| ApiError::NotFound(format!("no service {id:?}")))?;
    let command = service
        .command
        .as_ref()
        .ok_or_else(|| ApiError::BadRequest(format!("{id} is passive: no command")))?;
    let settings = &service.settings;
    let remote = crate::remote(&hub.config(), &service);
    Ok(Json(
        run_once(
            &hub,
            command,
            remote.as_ref(),
            settings.timeout,
            settings.format,
            &settings.thresholds,
        )
        .await?,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunBody {
    command: String,
    #[serde(default)]
    shell: bool,
    #[serde(default)]
    format: watcher_core::Format,
    /// Default 60s.
    timeout: Option<String>,
}

/// `POST /run-check`: runs any command as a check, with the check
/// directories in `PATH`, and returns the result. Admins only.
pub(crate) async fn run_check(
    State(hub): Shared,
    user: User,
    Json(body): Json<RunBody>,
) -> Result<Json<RunResult>, ApiError> {
    let who = require_admin(&hub, &user)?;
    let command = match body.shell {
        true => watcher_exec::CommandLine::Shell(body.command.clone()),
        false => watcher_exec::CommandLine::Argv(
            shell_words::split(&body.command)
                .map_err(|e| ApiError::BadRequest(format!("cannot split command: {e}")))?,
        ),
    };
    if matches!(&command, watcher_exec::CommandLine::Argv(a) if a.is_empty()) {
        return Err(ApiError::BadRequest("command is empty".into()));
    }
    let timeout = match &body.timeout {
        Some(t) => humantime::parse_duration(t)
            .map_err(|e| ApiError::BadRequest(format!("invalid timeout {t:?}: {e}")))?,
        None => Duration::from_secs(60),
    };
    info!("{who}: run-check {}", body.command);
    Ok(Json(
        run_once(&hub, &command, None, timeout, body.format, &BTreeMap::new()).await?,
    ))
}

/// Results per minute, for the process view.
#[derive(Default)]
pub(crate) struct Counter {
    /// (unix minute, results in it), oldest first, the last 15 minutes.
    minutes: std::collections::VecDeque<(u64, u64)>,
}

impl Counter {
    pub fn add(&mut self, now: SystemTime) {
        let minute = minute(now);
        match self.minutes.back_mut() {
            Some((m, n)) if *m == minute => *n += 1,
            _ => self.minutes.push_back((minute, 1)),
        }
        while self.minutes.front().is_some_and(|(m, _)| *m + 15 < minute) {
            self.minutes.pop_front();
        }
    }

    /// Results in the last `span`, by whole minutes.
    pub fn since(&self, now: SystemTime, span: Duration) -> u64 {
        let first = minute(now).saturating_sub(span.as_secs() / 60 - 1);
        self.minutes
            .iter()
            .filter(|(m, _)| *m >= first)
            .map(|(_, n)| n)
            .sum()
    }
}

fn minute(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 60
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_sums_recent_minutes() {
        let mut c = Counter::default();
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(60 * 1000);
        c.add(t0);
        c.add(t0 + Duration::from_secs(5));
        c.add(t0 + Duration::from_secs(120));
        let now = t0 + Duration::from_secs(130);
        assert_eq!(c.since(now, Duration::from_secs(60)), 1);
        assert_eq!(c.since(now, Duration::from_secs(300)), 3);
        c.add(t0 + Duration::from_secs(60 * 30));
        assert_eq!(c.minutes.len(), 1);
    }
}
