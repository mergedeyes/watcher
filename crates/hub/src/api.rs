//! HTTP API, version 1. All responses are JSON; errors are
//! `{"error": "..."}` with a matching status code.

use crate::view::{Counts, HostView, ServiceDetail, ServiceView};
use crate::{Hub, Update};
use axum::extract::{Path, Query, State as AxState};
use axum::http::StatusCode;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
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
use watcher_store::{EventFilter, Points, Series, StateEvent, Store};

type Shared = AxState<Arc<Hub>>;

pub fn router(hub: Arc<Hub>) -> Router {
    Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/hosts", get(hosts))
        .route("/api/v1/hosts/{host}", get(host))
        .route("/api/v1/services", get(services))
        .route("/api/v1/services/{host}/{service}", get(service))
        .route("/api/v1/services/{host}/{service}/result", post(submit))
        .route("/api/v1/services/{host}/{service}/series", get(series))
        .route("/api/v1/series/{id}/points", get(points))
        .route("/api/v1/events", get(events))
        .route("/api/v1/stream", get(stream))
        .fallback(|| async { ApiError::NotFound("no such endpoint".into()) })
        .with_state(hub)
}

#[derive(Debug)]
pub enum ApiError {
    NotFound(String),
    BadRequest(String),
    NoDatabase,
    Unavailable(String),
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            ApiError::NotFound(m) => (StatusCode::NOT_FOUND, m),
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            ApiError::NoDatabase => (
                StatusCode::SERVICE_UNAVAILABLE,
                "no database configured; history is not available".into(),
            ),
            ApiError::Unavailable(m) => (StatusCode::SERVICE_UNAVAILABLE, m),
            ApiError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        };
        (status, Json(json!({ "error": msg }))).into_response()
    }
}

impl From<watcher_store::Error> for ApiError {
    fn from(err: watcher_store::Error) -> Self {
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
    }))
}

/// All services as views, in config order.
fn service_views(hub: &Hub) -> Vec<ServiceView> {
    let live = hub.live.read().unwrap_or_else(|e| e.into_inner());
    live.config
        .services
        .iter()
        .map(|s| ServiceView::new(&live.config, s, live.services.get(&s.id)))
        .collect()
}

async fn hosts(AxState(hub): Shared) -> Json<Vec<HostView>> {
    let views = service_views(&hub);
    let config = hub
        .live
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .config
        .clone();
    let hosts = config
        .hosts
        .iter()
        .map(|h| {
            let own: Vec<&ServiceView> = views.iter().filter(|v| v.host == h.name).collect();
            HostView::new(h, &own)
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
    Ok(Json(json!({
        "host": HostView::new(host, &refs),
        "services": views,
        "origin": host.origin.to_string(),
    })))
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
    let live = hub.live.read().unwrap_or_else(|e| e.into_inner());
    let id = format!("{host}/{name}");
    let service = live
        .config
        .services
        .iter()
        .find(|s| s.id == id)
        .ok_or_else(|| ApiError::NotFound(format!("no service {id:?}")))?;
    Ok(Json(ServiceDetail::new(
        &live.config,
        service,
        live.services.get(&id),
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
    let live = hub.live.read().unwrap_or_else(|e| e.into_inner());
    let service = live
        .config
        .services
        .iter()
        .find(|s| s.id == id)
        .ok_or_else(|| ApiError::NotFound(format!("no service {id:?}")))?;
    Ok(Json(ServiceDetail::new(
        &live.config,
        service,
        live.services.get(&id),
    )))
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
    #[serde(default)]
    hard: bool,
    limit: Option<i64>,
}

async fn events(AxState(hub): Shared, Query(q): Query<EventsQuery>) -> ApiResult<Vec<StateEvent>> {
    let before = q
        .before
        .as_deref()
        .map(|s| parse_time(s, SystemTime::now()))
        .transpose()?;
    let filter = EventFilter {
        host: q.host,
        service: q.service,
        before,
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
fn parse_time(s: &str, now: SystemTime) -> Result<SystemTime, ApiError> {
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
