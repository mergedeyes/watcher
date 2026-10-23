//! HTTP API, version 1. All responses are JSON; errors are
//! `{"error": "..."}` with a matching status code.

use crate::view::{Counts, HostFlags, HostView, ServiceDetail, ServiceView};
use crate::{Hub, Update};
use crate::{auth, config, ops};
use axum::extract::{Path, Query, State as AxState};
use axum::http::StatusCode;
use axum::middleware;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::oneshot;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use watcher_core::{RawOutput, State, Termination};
use watcher_exec::scheduler::Control;
use watcher_store::{EventFilter, FlagKind, Points, Series, StateEvent, Store};

type Shared = AxState<Arc<Hub>>;

pub fn router(hub: Arc<Hub>) -> Router {
    Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/openapi.yaml", get(openapi))
        .route("/api/v1/hosts", get(hosts))
        .route("/api/v1/hosts/{host}", get(host))
        .route("/api/v1/services", get(services))
        .route("/api/v1/services/{host}/{service}", get(service))
        .route("/api/v1/services/{host}/{service}/result", post(submit))
        .route(
            "/api/v1/services/{host}/{service}/run",
            post(ops::run_service),
        )
        .route("/api/v1/services/{host}/{service}/series", get(series))
        .route("/api/v1/series/{id}/points", get(points))
        .route("/api/v1/events", get(events))
        .route("/api/v1/actions/{action}", post(ops::action))
        .route("/api/v1/flags", get(ops::flags))
        .route("/api/v1/downtimes", get(ops::downtimes))
        .route("/api/v1/downtimes/{id}", delete(ops::cancel_downtime))
        .route("/api/v1/process", get(ops::process).put(ops::set_process))
        .route("/api/v1/scheduler", get(ops::scheduler))
        .route("/api/v1/run-check", post(ops::run_check))
        .route("/api/v1/users", get(auth::users).post(auth::create_user))
        .route(
            "/api/v1/users/{name}",
            put(auth::update_user).delete(auth::delete_user),
        )
        .route("/api/v1/groups", get(groups))
        .route("/api/v1/stream", get(stream))
        .route("/api/v1/auth/status", get(auth::status))
        .route("/api/v1/auth/login", post(auth::login))
        .route("/api/v1/auth/logout", post(auth::logout))
        .route("/api/v1/tokens", get(auth::tokens).post(auth::create_token))
        .route("/api/v1/tokens/{id}", delete(auth::delete_token))
        .route("/api/v1/config", get(config::overview))
        .route("/api/v1/config/files/{*path}", get(config::file))
        .route("/api/v1/config/objects", get(config::objects))
        .route(
            "/api/v1/config/objects/{kind}/{name}",
            get(config::object)
                .put(config::put_object)
                .delete(config::delete_object),
        )
        .route("/api/v1/config/reload", post(config::reload))
        .route("/api/v1/config/match", post(config::matches))
        .route("/api/v1/config/scripts", get(config::scripts))
        .route("/api/v1/config/tree/{kind}/{*name}", get(config::tree))
        .route("/api/v1/config/edit", post(config::edit))
        .route("/api/v1/config/edit-many", post(config::edit_many))
        .route("/api/v1/config/preview", post(config::preview))
        .route("/api/v1/config/apply", post(config::apply))
        // Unknown /api paths get a JSON 404; everything else is the web UI.
        .fallback(crate::ui::serve)
        .layer(middleware::from_fn_with_state(hub.clone(), auth::require))
        .with_state(hub)
}

#[derive(Debug)]
pub enum ApiError {
    NotFound(String),
    BadRequest(String),
    /// Not logged in, or wrong credentials.
    Unauthorized,
    Forbidden(String),
    Conflict(String),
    TooManyRequests(String),
    NoDatabase,
    Unavailable(String),
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            ApiError::NotFound(m) => (StatusCode::NOT_FOUND, m),
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            ApiError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "unauthorized: session or Bearer token required".into(),
            ),
            ApiError::Forbidden(m) => (StatusCode::FORBIDDEN, m),
            ApiError::Conflict(m) => (StatusCode::CONFLICT, m),
            ApiError::TooManyRequests(m) => (StatusCode::TOO_MANY_REQUESTS, m),
            ApiError::NoDatabase => (
                StatusCode::SERVICE_UNAVAILABLE,
                "no database configured".into(),
            ),
            ApiError::Unavailable(m) => (StatusCode::SERVICE_UNAVAILABLE, m),
            ApiError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        };
        (status, Json(json!({ "error": msg }))).into_response()
    }
}

impl From<watcher_store::Error> for ApiError {
    fn from(err: watcher_store::Error) -> Self {
        if let watcher_store::Error::Conflict(msg) = err {
            return ApiError::Conflict(msg);
        }
        tracing::warn!("database query failed: {err}");
        ApiError::Internal("database query failed".into())
    }
}

type ApiResult<T> = Result<Json<T>, ApiError>;

fn store(hub: &Hub) -> Result<&Store, ApiError> {
    hub.store.as_deref().ok_or(ApiError::NoDatabase)
}

async fn health(AxState(hub): Shared) -> Json<serde_json::Value> {
    let services = hub
        .live
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .config
        .services
        .len();
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_s": hub.started.elapsed().unwrap_or_default().as_secs(),
        "services": services,
        "database": hub.store.as_ref().map(|s| s.backend().to_string()),
        "instance": hub.instance.get(),
    }))
}

/// The API description; `crates/hub/openapi.yaml`.
const OPENAPI: &str = include_str!("../openapi.yaml");

async fn openapi() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/yaml")],
        OPENAPI,
    )
}

/// All services as views, in config order.
fn service_views(hub: &Hub) -> Vec<ServiceView> {
    let live = hub.read();
    let now = SystemTime::now();
    live.config
        .services
        .iter()
        .map(|s| {
            let status = live.ops.status(&live.config, s, now);
            ServiceView::new(&live.config, s, live.services.get(&s.id), &status)
        })
        .collect()
}

fn host_flags(hub: &Hub, host: &watcher_config::Host) -> HostFlags {
    let live = hub.read();
    let ops = &live.ops;
    HostFlags {
        acknowledged: ops.flag(FlagKind::Ack, &host.name).is_some(),
        muted: ops.flag(FlagKind::Mute, &host.name).is_some(),
        checks_enabled: ops.checks && ops.flag(FlagKind::NoChecks, &host.name).is_none(),
        downtime: !ops
            .downtimes_of(&live.config, host, None, SystemTime::now())
            .is_empty(),
    }
}

async fn hosts(AxState(hub): Shared) -> Json<Vec<HostView>> {
    let views = service_views(&hub);
    let config = hub.config();
    let hosts = config
        .hosts
        .iter()
        .map(|h| {
            let own: Vec<&ServiceView> = views.iter().filter(|v| v.host == h.name).collect();
            HostView::new(h, &own, &host_flags(&hub, h))
        })
        .collect();
    Json(hosts)
}

async fn host(AxState(hub): Shared, Path(name): Path<String>) -> ApiResult<serde_json::Value> {
    let config = hub
        .live
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .config
        .clone();
    let host = config
        .hosts
        .iter()
        .find(|h| h.name == name)
        .ok_or_else(|| ApiError::NotFound(format!("no host {name:?}")))?;
    let views: Vec<ServiceView> = service_views(&hub)
        .into_iter()
        .filter(|v| v.host == name)
        .collect();
    let refs: Vec<&ServiceView> = views.iter().collect();
    let flags: Vec<watcher_store::Flag> = hub
        .read()
        .ops
        .flags
        .values()
        .filter(|f| f.target == name)
        .cloned()
        .collect();
    Ok(Json(json!({
        "host": HostView::new(host, &refs, &host_flags(&hub, host)),
        "flags": flags,
        "services": views,
        "origin": host.origin.to_string(),
    })))
}

/// Host, service and downtime groups with their titles; host and service
/// group members are in the host and service views (`groups`).
async fn groups(AxState(hub): Shared) -> Json<serde_json::Value> {
    let config = hub.config();
    let list = |groups: &[watcher_config::Group]| -> Vec<serde_json::Value> {
        groups
            .iter()
            .map(|g| json!({ "name": g.name, "title": g.title }))
            .collect()
    };
    Json(json!({
        "hostgroups": list(&config.hostgroups),
        "servicegroups": list(&config.servicegroups),
        "downtimegroups": config
            .downtimegroups
            .iter()
            .map(|g| json!({
                "name": g.name,
                "title": g.title,
                "schedule": g.schedule.iter().map(ToString::to_string).collect::<Vec<_>>(),
            }))
            .collect::<Vec<_>>(),
    }))
}

#[derive(Debug, Deserialize)]
struct ServiceFilter {
    host: Option<String>,
    /// Comma-separated: `ok,warning,critical,unknown,pending`.
    state: Option<String>,
    /// Only services that are not OK (and not pending).
    #[serde(default)]
    problems: bool,
    /// Only confirmed (hard) states.
    #[serde(default)]
    hard: bool,
}

async fn services(
    AxState(hub): Shared,
    Query(f): Query<ServiceFilter>,
) -> ApiResult<serde_json::Value> {
    let states: Option<Vec<Option<State>>> = match &f.state {
        None => None,
        Some(list) => Some(
            list.split(',')
                .map(|s| match s.trim() {
                    "pending" => Ok(None),
                    other => State::parse(other)
                        .map(Some)
                        .ok_or_else(|| ApiError::BadRequest(format!("unknown state {other:?}"))),
                })
                .collect::<Result<_, _>>()?,
        ),
    };
    let views: Vec<ServiceView> = service_views(&hub)
        .into_iter()
        .filter(|v| f.host.as_ref().is_none_or(|h| *h == v.host))
        .filter(|v| states.as_ref().is_none_or(|s| s.contains(&v.state)))
        .filter(|v| !f.problems || v.state.is_some_and(|s| s != State::Ok))
        .filter(|v| !f.hard || v.state_type == Some(watcher_core::StateType::Hard))
        .collect();
    let mut counts = Counts::default();
    for v in &views {
        counts.add(v.state);
    }
    Ok(Json(json!({ "counts": counts, "services": views })))
}

async fn service(
    AxState(hub): Shared,
    Path((host, name)): Path<(String, String)>,
) -> ApiResult<ServiceDetail> {
    let id = format!("{host}/{name}");
    detail(&hub, &id).ok_or_else(|| ApiError::NotFound(format!("no service {id:?}")))
}

fn detail(hub: &Hub, id: &str) -> Option<Json<ServiceDetail>> {
    let live = hub.read();
    let service = live.config.service(id)?;
    let host = &live.config.host(service).name;
    let status = live.ops.status(&live.config, service, SystemTime::now());
    let flags = live
        .ops
        .flags
        .values()
        .filter(|f| f.target == *id || f.target == *host)
        .cloned()
        .collect();
    Some(Json(ServiceDetail::new(
        &live.config,
        service,
        live.services.get(id),
        &status,
        flags,
    )))
}

/// A passive result. Give `exit_code` (Nagios semantics) or `state`.
#[derive(Debug, Deserialize)]
struct ResultBody {
    exit_code: Option<i32>,
    state: Option<String>,
    /// Plugin output, parsed like stdout of an active check.
    #[serde(default)]
    output: String,
}

async fn submit(
    AxState(hub): Shared,
    Path((host, name)): Path<(String, String)>,
    Json(body): Json<ResultBody>,
) -> ApiResult<ServiceDetail> {
    let exit = match (body.exit_code, body.state.as_deref()) {
        (Some(code), None) => code,
        (None, Some(state)) => State::parse(state)
            .ok_or_else(|| ApiError::BadRequest(format!("unknown state {state:?}")))?
            .exit_code(),
        _ => {
            return Err(ApiError::BadRequest(
                "give either exit_code or state".into(),
            ));
        }
    };
    let id = format!("{host}/{name}");
    let (reply, rx) = oneshot::channel();
    let output = RawOutput {
        termination: Termination::Exited(exit),
        stdout: body.output,
        stderr: String::new(),
    };
    let control = Control::Submit {
        id: id.clone(),
        output,
        reply,
    };
    let unavailable = || ApiError::Unavailable("the scheduler is not running".into());
    hub.control.send(control).await.map_err(|_| unavailable())?;
    let event = rx.await.map_err(|_| unavailable())?;
    if event.is_none() {
        return Err(ApiError::NotFound(format!("no service {id:?}")));
    }
    // The scheduler also forwarded the event to the run loop, which may not
    // have recorded it yet; record it here so the response is current.
    if let Some(event) = &event {
        hub.record(event);
    }
    detail(&hub, &id).ok_or_else(|| ApiError::NotFound(format!("no service {id:?}")))
}

async fn series(
    AxState(hub): Shared,
    Path((host, name)): Path<(String, String)>,
) -> ApiResult<Vec<Series>> {
    store(&hub)?
        .series(&host, &name)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("no service \"{host}/{name}\"")))
}

#[derive(Debug, Deserialize)]
struct PointsQuery {
    /// RFC 3339 time, unix seconds, or a duration back from now (`6h`).
    /// Defaults to 1h ago.
    from: Option<String>,
    /// Like `from`; defaults to now.
    to: Option<String>,
    /// Upper bound for the number of buckets; defaults to 600.
    points: Option<u32>,
}

async fn points(
    AxState(hub): Shared,
    Path(id): Path<i32>,
    Query(q): Query<PointsQuery>,
) -> ApiResult<Points> {
    let now = SystemTime::now();
    let from = q
        .from
        .as_deref()
        .map_or(Ok(now - Duration::from_secs(3600)), |s| parse_time(s, now))?;
    let to = q.to.as_deref().map_or(Ok(now), |s| parse_time(s, now))?;
    if from >= to {
        return Err(ApiError::BadRequest("from must be before to".into()));
    }
    let max_points = q.points.unwrap_or(600).clamp(1, 10_000);
    store(&hub)?
        .points(id, from, to, max_points)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("no series {id}")))
}

#[derive(Debug, Deserialize)]
struct EventsQuery {
    host: Option<String>,
    service: Option<String>,
    /// Only events before this time (see `from` of points), for paging.
    before: Option<String>,
    /// Only events at or after this time, like `before`.
    since: Option<String>,
    /// Comma-separated: `service,host,flapping`.
    #[serde(rename = "type")]
    kind: Option<String>,
    /// Comma-separated states changed to: `ok,warning,critical,unknown,up,down`.
    state: Option<String>,
    /// `soft` or `hard`.
    state_type: Option<String>,
    /// Only hard changes: confirmed problems and their recoveries.
    #[serde(default)]
    hard: bool,
    limit: Option<i64>,
}

/// Splits a comma-separated parameter, checking every item against `known`.
fn list_param(name: &str, value: Option<&str>, known: &[&str]) -> Result<Vec<String>, ApiError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| match known.contains(&s) {
            true => Ok(s.to_string()),
            false => Err(ApiError::BadRequest(format!("{name}: unknown value {s:?}"))),
        })
        .collect()
}

async fn events(AxState(hub): Shared, Query(q): Query<EventsQuery>) -> ApiResult<Vec<StateEvent>> {
    let now = SystemTime::now();
    let time = |s: &Option<String>| s.as_deref().map(|s| parse_time(s, now)).transpose();
    let mut kinds = list_param("type", q.kind.as_deref(), &["service", "host", "flapping"])?;
    if let Some(i) = kinds.iter().position(|k| k == "flapping") {
        kinds.splice(i..=i, ["flap_start".into(), "flap_stop".into()]);
    }
    let states = ["ok", "warning", "critical", "unknown", "up", "down"];
    let state_type = list_param("state_type", q.state_type.as_deref(), &["soft", "hard"])?;
    if state_type.len() > 1 {
        return Err(ApiError::BadRequest("state_type: soft or hard".into()));
    }
    let filter = EventFilter {
        host: q.host,
        service: q.service,
        before: time(&q.before)?,
        since: time(&q.since)?,
        kinds,
        states: list_param("state", q.state.as_deref(), &states)?,
        state_type: state_type.into_iter().next(),
        hard_only: q.hard,
        limit: q.limit.unwrap_or(100),
    };
    Ok(Json(store(&hub)?.events(&filter).await?))
}

/// Server-sent events: `result` with a service view after every check,
/// `reload` after a configuration reload, `lagged` if the client fell
/// behind and should refetch.
async fn stream(
    AxState(hub): Shared,
) -> Sse<impl tokio_stream::Stream<Item = Result<SseEvent, Infallible>>> {
    let updates = BroadcastStream::new(hub.updates.subscribe()).map(|msg| {
        Ok(match msg {
            Ok(Update::Result(json)) => SseEvent::default().event("result").data(&*json),
            Ok(Update::Reload) => SseEvent::default().event("reload").data("{}"),
            Err(BroadcastStreamRecvError::Lagged(n)) => {
                SseEvent::default().event("lagged").data(n.to_string())
            }
        })
    });
    Sse::new(updates).keep_alive(KeepAlive::default())
}

/// RFC 3339, unix seconds, `now`, or a duration back from `now` (`6h`).
pub(crate) fn parse_time(s: &str, now: SystemTime) -> Result<SystemTime, ApiError> {
    let s = s.trim();
    if s == "now" {
        return Ok(now);
    }
    if let Ok(secs) = s.parse::<f64>()
        && secs.is_finite()
        && secs >= 0.0
    {
        return Ok(SystemTime::UNIX_EPOCH + Duration::from_secs_f64(secs));
    }
    if let Ok(t) = humantime::parse_rfc3339_weak(s) {
        return Ok(t);
    }
    if let Ok(d) = humantime::parse_duration(s.trim_start_matches('-')) {
        return now
            .checked_sub(d)
            .ok_or_else(|| ApiError::BadRequest(format!("time out of range: {s:?}")));
    }
    Err(ApiError::BadRequest(format!(
        "invalid time {s:?} (use RFC 3339, unix seconds, or a duration like 6h)"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every route is described in openapi.yaml, and nothing else.
    #[test]
    fn openapi_lists_every_route() {
        let source = include_str!("api.rs");
        // `.route("/api/v1/...` with the path on the same or the next line,
        // outside this test.
        let router = source.split("#[cfg(test)]").next().unwrap();
        let mut routes: Vec<String> = router
            .split(".route(")
            .skip(1)
            .filter_map(|r| r.trim_start().strip_prefix("\"/api/v1"))
            .map(|r| r.split('"').next().unwrap().replace("{*", "{"))
            .collect();
        routes.sort();
        let mut paths: Vec<String> = OPENAPI
            .lines()
            .filter(|l| l.starts_with("  /") && l.ends_with(':'))
            .map(|l| l.trim().trim_end_matches(':').to_string())
            .collect();
        paths.sort();
        assert_eq!(routes, paths);
    }

    #[test]
    fn parses_times() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
        assert_eq!(parse_time("now", now).unwrap(), now);
        assert_eq!(
            parse_time("1h", now).unwrap(),
            now - Duration::from_secs(3600)
        );
        assert_eq!(
            parse_time("-1h", now).unwrap(),
            now - Duration::from_secs(3600)
        );
        assert_eq!(
            parse_time("1500", now).unwrap(),
            SystemTime::UNIX_EPOCH + Duration::from_secs(1500)
        );
        assert_eq!(
            parse_time("1970-01-01T00:00:10Z", now).unwrap(),
            SystemTime::UNIX_EPOCH + Duration::from_secs(10)
        );
        assert!(parse_time("yesterday", now).is_err());
    }
}
