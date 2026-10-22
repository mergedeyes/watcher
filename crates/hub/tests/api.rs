use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc;
use tower::ServiceExt;
use watcher_config::{Config, load_dir};
use watcher_core::{Format, RawOutput, ServiceState, Termination, evaluate};
use watcher_exec::scheduler::{Event, schedule};
use watcher_hub::{AuthSettings, Hub, jobs};

const CONFIG: &str = r#"
[[host]]
name = "web01"
tags = ["web"]

[[host]]
name = "db01"

[[check]]
name = "http"
apply_to = '"web" in host.tags'
command = "check_http -H $host.address$"
interval = "1h"

[[check]]
name = "backup"
passive = true
hosts = ["db01"]
"#;

fn config() -> Arc<Config> {
    let dir: PathBuf = std::env::temp_dir().join(format!(
        "watcher-hub-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("conf.toml"), CONFIG).unwrap();
    let loaded = load_dir(&dir).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    Arc::new(loaded.config)
}

/// A hub with a running scheduler (its active check runs only hourly).
fn hub() -> Arc<Hub> {
    let config = config();
    let (control, control_rx) = mpsc::channel(8);
    let (events_tx, mut events) = mpsc::channel::<Event>(64);
    let hub = Hub::new(
        config.clone(),
        HashMap::new(),
        control,
        None,
        AuthSettings::OFF,
    );
    tokio::spawn(schedule(
        jobs(&config),
        HashMap::new(),
        4,
        control_rx,
        events_tx,
    ));
    let recorder = hub.clone();
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            recorder.record(&event);
        }
    });
    hub
}

async fn call(hub: &Arc<Hub>, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(json) => {
            req = req.header("content-type", "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    let resp = hub
        .clone()
        .router()
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

fn event(id: &str, exit: i32, stdout: &str) -> Event {
    let raw = RawOutput {
        termination: Termination::Exited(exit),
        stdout: stdout.into(),
        stderr: String::new(),
    };
    let result = evaluate(&raw, Format::Auto, &Default::default());
    let (state, change) = ServiceState::first(result.state, 3, SystemTime::now());
    Event {
        id: id.into(),
        result,
        duration: Duration::from_millis(5),
        state,
        change: Some(change),
    }
}

#[tokio::test]
async fn health_and_lists() {
    let hub = hub();
    let (status, health) = call(&hub, "GET", "/api/v1/health", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(health["services"], 2);
    assert_eq!(health["database"], Value::Null);

    let (_, list) = call(&hub, "GET", "/api/v1/services", None).await;
    assert_eq!(list["counts"]["pending"], 2);
    assert_eq!(list["services"][0]["state"], Value::Null);

    hub.record(&event("web01/http", 2, "HTTP CRITICAL - down | time=1.5s"));
    let (_, problems) = call(&hub, "GET", "/api/v1/services?problems=true", None).await;
    assert_eq!(problems["services"].as_array().unwrap().len(), 1);
    assert_eq!(problems["services"][0]["state"], "critical");
    assert_eq!(problems["services"][0]["state_type"], "soft");
    let (_, hard) = call(
        &hub,
        "GET",
        "/api/v1/services?problems=true&hard=true",
        None,
    )
    .await;
    assert_eq!(hard["services"].as_array().unwrap().len(), 0);
    let (_, by_state) = call(&hub, "GET", "/api/v1/services?state=pending", None).await;
    assert_eq!(by_state["services"][0]["service"], "backup");
    let (status, _) = call(&hub, "GET", "/api/v1/services?state=bad", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (_, hosts) = call(&hub, "GET", "/api/v1/hosts", None).await;
    assert_eq!(hosts[0]["name"], "web01");
    assert_eq!(hosts[0]["state"], "critical");
    assert_eq!(hosts[1]["state"], Value::Null);

    let (status, web) = call(&hub, "GET", "/api/v1/hosts/web01", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(web["services"][0]["output"], "HTTP CRITICAL - down");
    let (status, err) = call(&hub, "GET", "/api/v1/hosts/nope", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(err["error"], "no host \"nope\"");

    let (_, detail) = call(&hub, "GET", "/api/v1/services/web01/http", None).await;
    assert_eq!(detail["metrics"][0]["name"], "time");
    assert_eq!(detail["metrics"][0]["value"], 1.5);
    assert_eq!(detail["check"]["command"], "check_http -H web01");
    assert_eq!(detail["check"]["interval_s"], 3600.0);
    assert_eq!(detail["duration_ms"], 5.0);
}

#[tokio::test]
async fn passive_results() {
    let hub = hub();
    let body = json!({"exit_code": 2, "output": "BACKUP CRITICAL - failed | size=0B"});
    let (status, detail) = call(
        &hub,
        "POST",
        "/api/v1/services/db01/backup/result",
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["state"], "critical");
    assert_eq!(detail["output"], "BACKUP CRITICAL - failed");
    assert_eq!(detail["passive"], true);
    assert_eq!(detail["check"]["command"], Value::Null);

    let body = json!({"state": "ok", "output": "fine"});
    let (_, detail) = call(
        &hub,
        "POST",
        "/api/v1/services/db01/backup/result",
        Some(body),
    )
    .await;
    assert_eq!(detail["state"], "ok");

    let body = json!({"state": "ok"});
    let (status, _) = call(
        &hub,
        "POST",
        "/api/v1/services/db01/nope/result",
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let body = json!({"output": "no state"});
    let (status, _) = call(
        &hub,
        "POST",
        "/api/v1/services/db01/backup/result",
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn history_needs_a_database() {
    let hub = hub();
    for uri in [
        "/api/v1/events",
        "/api/v1/services/web01/http/series",
        "/api/v1/series/1/points",
    ] {
        let (status, err) = call(&hub, "GET", uri, None).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{uri}");
        assert!(err["error"].as_str().unwrap().contains("no database"));
    }
    let (status, _) = call(&hub, "GET", "/api/v1/nope", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn stream_sends_results() {
    let hub = hub();
    let resp = hub
        .clone()
        .router()
        .oneshot(Request::get("/api/v1/stream").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.headers()["content-type"], "text/event-stream");
    let mut body = resp.into_body();

    hub.record(&event("web01/http", 0, "HTTP OK"));
    let frame = tokio::time::timeout(Duration::from_secs(5), body.frame())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let text = String::from_utf8(frame.into_data().unwrap().to_vec()).unwrap();
    assert!(text.starts_with("event: result\ndata: {"), "{text}");
    assert!(text.contains(r#""output":"HTTP OK""#), "{text}");
}

#[tokio::test]
async fn serves_the_web_ui_for_app_routes() {
    let hub = hub();
    for uri in ["/", "/hosts", "/services/web01/http"] {
        let resp = hub
            .clone()
            .router()
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{uri}");
        let ct = resp.headers()["content-type"].to_str().unwrap().to_string();
        assert!(ct.starts_with("text/html"), "{uri}: {ct}");
    }
    // API paths never fall back to the UI.
    let (status, err) = call(&hub, "GET", "/api/v1/nope", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(err["error"], "no such endpoint");
}
