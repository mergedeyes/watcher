//! HTTP API, version 1. All responses are JSON; errors are
//! `{"error": "..."}` with a matching status code.

use crate::list::{self, HostList, HostQuery, ServiceList, ServiceQuery};
use crate::view::{SeriesView, ServiceDetail};
use crate::{Hub, auth, config, ops, ssh, stream};
use axum::extract::{Path, Query, State as AxState};
use axum::http::StatusCode;
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::oneshot;
use watcher_core::{RawOutput, State, Termination};
use watcher_exec::scheduler::Control;
use watcher_store::{EventFilter, FlagKind, Points, StateEvent, Store};

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
            "/api/v1/ssh/secrets",
            get(ssh::secrets).post(ssh::create_secret),
        )
        .route("/api/v1/ssh/secrets/{name}", delete(ssh::delete_secret))
        .route("/api/v1/ssh/hostkeys", get(ssh::host_keys))
        .route(
            "/api/v1/ssh/hostkeys/{address}/{port}",
            delete(ssh::forget_host_key),
        )
        .route(
            "/api/v1/ssh/hostkeys/{address}/{port}/accept",
            post(ssh::accept_host_key),
        )
        .route(
            "/api/v1/users/{name}",
            put(auth::update_user).delete(auth::delete_user),
        )
        .route("/api/v1/groups", get(groups))
        .route("/api/v1/summary", get(summary))
        .route("/api/v1/names", get(names))
        .route("/api/v1/stream", get(stream::stream))
        .route("/api/v1/stream/{id}", put(stream::set_watch))
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

/// `GET /hosts`: filtered, sorted and paged.
async fn hosts(AxState(hub): Shared, Query(q): Query<HostQuery>) -> ApiResult<HostList> {
    Ok(Json(list::hosts(&hub.read(), &q)?))
}

async fn host(AxState(hub): Shared, Path(name): Path<String>) -> ApiResult<serde_json::Value> {
    let live = hub.read();
    let (index, host) = live
        .config
        .host_named(&name)
        .ok_or_else(|| ApiError::NotFound(format!("no host {name:?}")))?;
    let (view, services) = list::host_view(&live, index, SystemTime::now());
    let flags: Vec<watcher_store::Flag> = live
        .ops
        .flags
        .values()
        .filter(|f| f.target == name)
        .cloned()
        .collect();
    Ok(Json(json!({
        "host": view,
        "flags": flags,
        "services": services,
        "origin": host.origin.to_string(),
    })))
}

/// Host, service and downtime groups with their titles and status totals;
/// host and service group members are in the host and service views
/// (`groups`).
async fn groups(AxState(hub): Shared) -> Json<serde_json::Value> {
    let live = hub.read();
    let (mut hostgroups, mut servicegroups) = list::group_counts(&live);
    let config = &live.config;
    let list = |groups: &[watcher_config::Group],
                counts: &mut std::collections::HashMap<String, list::GroupCounts>|
     -> Vec<serde_json::Value> {
        groups
            .iter()
            .map(|g| {
                let mut out = json!({ "name": g.name, "title": g.title });
                let c = counts.remove(&g.name).unwrap_or_default();
                if let Some(hosts) = c.hosts {
                    out["hosts"] = json!(hosts);
                }
                out["services"] = json!(c.services);
                out
            })
            .collect()
    };
    Json(json!({
        "hostgroups": list(&config.hostgroups, &mut hostgroups),
        "servicegroups": list(&config.servicegroups, &mut servicegroups),
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

/// `GET /summary`: host and service totals, as in the stream's `summary`.
async fn summary(AxState(hub): Shared) -> Response {
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        hub.summary_json().to_string(),
    )
        .into_response()
}

/// `GET /names`: all host names and the distinct service names, for
/// suggestions.
async fn names(AxState(hub): Shared) -> Json<serde_json::Value> {
    let config = hub.config();
    let hosts: Vec<&str> = config.hosts.iter().map(|h| h.name.as_str()).collect();
    let services: BTreeSet<&str> = config.services.iter().map(|s| s.name.as_str()).collect();
    Json(json!({ "hosts": hosts, "services": services }))
}

/// `GET /services`: filtered, sorted and paged.
async fn services(AxState(hub): Shared, Query(q): Query<ServiceQuery>) -> ApiResult<ServiceList> {
    Ok(Json(list::services(&hub.read(), &q)?))
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
    // A host acknowledgement covers only its host check.
    let host_ack = live.config.is_host_check(service);
    let flags = live
        .ops
        .flags
        .values()
        .filter(|f| f.target == *id || (f.target == *host && (f.kind != FlagKind::Ack || host_ack)))
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
) -> ApiResult<Vec<SeriesView>> {
    let id = format!("{host}/{name}");
    let list = store(&hub)?
        .series(&host, &name)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("no service {id:?}")))?;
    let live = hub.read();
    let service = live.services.get(&id);
    Ok(Json(
        list.into_iter()
            .map(|s| SeriesView::new(s, service))
            .collect(),
    ))
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
    /// Comma-separated: `service,host,flapping,ack,downtime`.
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

/// An alert with the service's current title.
#[derive(Debug, Serialize)]
struct EventView {
    #[serde(flatten)]
    event: StateEvent,
    title: Option<String>,
}

async fn events(AxState(hub): Shared, Query(q): Query<EventsQuery>) -> ApiResult<Vec<EventView>> {
    let now = SystemTime::now();
    let time = |s: &Option<String>| s.as_deref().map(|s| parse_time(s, now)).transpose();
    let known = ["service", "host", "flapping", "ack", "downtime"];
    let mut kinds = list_param("type", q.kind.as_deref(), &known)?;
    for (name, parts) in [
        ("flapping", ["flap_start", "flap_stop"]),
        ("downtime", ["downtime_start", "downtime_end"]),
    ] {
        if let Some(i) = kinds.iter().position(|k| k == name) {
            kinds.splice(i..=i, parts.map(String::from));
        }
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
    let list = store(&hub)?.events(&filter).await?;
    let config = hub.config();
    Ok(Json(
        list.into_iter()
            .map(|event| {
                let title = (!event.service.is_empty())
                    .then(|| config.service(&format!("{}/{}", event.host, event.service)))
                    .flatten()
                    .and_then(|s| s.title.clone());
                EventView { event, title }
            })
            .collect(),
    ))
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
