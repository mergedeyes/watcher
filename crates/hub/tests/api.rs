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
use watcher_config::{CheckDirs, Config, load_dir};
use watcher_core::{Format, RawOutput, ServiceState, Termination, evaluate};
use watcher_exec::scheduler::{Event, schedule};
use watcher_hub::{AuthSettings, Hub, jobs};

const CONFIG: &str = r#"
[check.http]
command = "check_http -H $host.address$"
interval = "1h"

[service.http]

[hostgroup.web]
services = ["http"]

[[host]]
name = "web01"
groups = ["web"]

[[host]]
name = "db01"
host_check = "backup"
  [host.service.backup]
  passive = true
  [host.service.dump]
  passive = true
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
        jobs(&config, &CheckDirs::default()),
        HashMap::new(),
        4,
        Arc::new(watcher_exec::ssh::Pool::unavailable("no SSH in tests")),
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
    let (state, change) = ServiceState::first(result.state, 3, false, SystemTime::now());
    Event {
        id: id.into(),
        result,
        duration: Duration::from_millis(5),
        state,
        change: Some(change),
        flap: None,
    }
}

#[tokio::test]
async fn health_and_lists() {
    let hub = hub();
    let (status, health) = call(&hub, "GET", "/api/v1/health", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(health["services"], 3);
    assert_eq!(health["database"], Value::Null);

    let (_, list) = call(&hub, "GET", "/api/v1/services", None).await;
    assert_eq!(list["counts"]["pending"], 3);
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
    // No host check: UP. The worst service state is separate.
    assert_eq!(hosts[0]["state"], "up");
    assert_eq!(hosts[0]["check"], Value::Null);
    assert_eq!(hosts[0]["worst"], "critical");
    // The host check has no result yet: pending.
    assert_eq!(hosts[1]["state"], Value::Null);
    assert_eq!(hosts[1]["check"], "backup");
    assert_eq!(hosts[1]["worst"], Value::Null);

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
    assert_eq!(detail["host_check"], true);

    // It is db01's host check: the host is DOWN (passive: 1 attempt, hard).
    let (_, db) = call(&hub, "GET", "/api/v1/hosts/db01", None).await;
    assert_eq!(db["host"]["state"], "down");
    assert_eq!(db["host"]["state_type"], "hard");
    assert_eq!(db["host"]["output"], "BACKUP CRITICAL - failed");
    // Other services of the host don't notify; the host check does.
    assert!(hub.host_down("db01/dump"));
    assert!(!hub.host_down("db01/backup"));
    assert!(!hub.host_down("web01/http"));

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

#[tokio::test]
async fn actions_flags_and_downtimes() {
    let hub = hub();
    hub.record(&event("web01/http", 2, "HTTP CRITICAL - down"));

    // Acknowledge: only problems.
    let (status, ack) = call(
        &hub,
        "POST",
        "/api/v1/actions/ack",
        Some(json!({ "services": ["web01/http", "db01/dump"], "comment": "on it" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{ack}");
    assert_eq!(ack["done"], json!(["web01/http"]));
    assert_eq!(ack["skipped"][0]["target"], "db01/dump");
    let (_, svc) = call(&hub, "GET", "/api/v1/services/web01/http", None).await;
    assert_eq!(svc["acknowledged"], true);
    assert_eq!(svc["flags"][0]["comment"], "on it");
    assert_eq!(
        hub.suppressed("web01/http", watcher_core::State::Critical)
            .as_deref(),
        Some("acknowledged")
    );
    // OK again: the acknowledgement ends, the recovery notifies.
    hub.record(&event("web01/http", 0, "HTTP OK"));
    let (_, svc) = call(&hub, "GET", "/api/v1/services/web01/http", None).await;
    assert_eq!(svc["acknowledged"], false);
    assert_eq!(hub.suppressed("web01/http", watcher_core::State::Ok), None);

    // Mute a host: its services are muted.
    let (_, mute) = call(
        &hub,
        "POST",
        "/api/v1/actions/mute",
        Some(json!({ "hosts": ["db01"] })),
    )
    .await;
    assert_eq!(mute["done"], json!(["db01"]));
    let (_, list) = call(&hub, "GET", "/api/v1/services?host=db01", None).await;
    assert!(
        list["services"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["muted"] == true)
    );
    assert_eq!(
        hub.suppressed("db01/dump", watcher_core::State::Critical)
            .as_deref(),
        Some("muted")
    );
    let (_, unmute) = call(
        &hub,
        "POST",
        "/api/v1/actions/unmute",
        Some(json!({ "hosts": ["db01"] })),
    )
    .await;
    assert_eq!(unmute["done"], json!(["db01"]));

    // A downtime for an hour, by duration.
    let (status, dt) = call(
        &hub,
        "POST",
        "/api/v1/actions/downtime",
        Some(json!({ "hosts": ["web01"], "duration": "1h", "comment": "patching" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{dt}");
    let id = dt["downtimes"][0]["id"].as_i64().unwrap();
    let (_, list) = call(&hub, "GET", "/api/v1/downtimes", None).await;
    assert_eq!(list[0]["active"], true);
    assert_eq!(list[0]["services"], 1);
    let (_, hosts) = call(&hub, "GET", "/api/v1/hosts", None).await;
    let web = hosts
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["name"] == "web01")
        .unwrap();
    assert_eq!(web["downtime"], true);
    assert!(
        hub.suppressed("web01/http", watcher_core::State::Critical)
            .unwrap()
            .starts_with("in downtime")
    );
    let (status, _) = call(&hub, "DELETE", &format!("/api/v1/downtimes/{id}"), None).await;
    assert_eq!(status, StatusCode::OK);
    let (_, list) = call(&hub, "GET", "/api/v1/downtimes", None).await;
    assert_eq!(list, json!([]));

    // Errors.
    let (status, err) = call(
        &hub,
        "POST",
        "/api/v1/actions/mute",
        Some(json!({ "hosts": ["nope"] })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{err}");
    let (status, _) = call(
        &hub,
        "POST",
        "/api/v1/actions/explode",
        Some(json!({ "hosts": ["web01"] })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(
        &hub,
        "POST",
        "/api/v1/actions/downtime",
        Some(json!({ "hosts": ["web01"] })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn host_ack_covers_only_the_host_check() {
    let hub = hub();
    hub.record(&event("db01/backup", 2, "BACKUP CRITICAL"));
    hub.record(&event("db01/dump", 2, "DUMP CRITICAL"));
    let (status, ack) = call(
        &hub,
        "POST",
        "/api/v1/actions/ack",
        Some(json!({ "hosts": ["db01"], "comment": "on it" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{ack}");
    assert_eq!(ack["done"], json!(["db01"]));
    let (_, check) = call(&hub, "GET", "/api/v1/services/db01/backup", None).await;
    assert_eq!(check["acknowledged"], true);
    assert_eq!(check["flags"][0]["target"], "db01");
    let (_, other) = call(&hub, "GET", "/api/v1/services/db01/dump", None).await;
    assert_eq!(other["acknowledged"], false);
    assert_eq!(other["flags"], json!([]));
}

#[tokio::test]
async fn recheck_process_and_schedule() {
    let hub = hub();
    // The http check runs hourly; recheck runs it now.
    let (status, done) = call(
        &hub,
        "POST",
        "/api/v1/actions/recheck",
        Some(json!({ "hosts": ["web01"] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["done"], json!(["web01/http"]));
    let mut ran = false;
    for _ in 0..50 {
        let (_, svc) = call(&hub, "GET", "/api/v1/services/web01/http", None).await;
        if !svc["last_check"].is_null() {
            ran = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(ran, "recheck didn't run the check");

    let (_, process) = call(&hub, "GET", "/api/v1/process", None).await;
    assert_eq!(process["notifications"], true);
    assert_eq!(process["scheduler"]["jobs"], 3);
    assert_eq!(process["scheduler"]["passive"], 2);
    assert!(process["results"]["last_1m"].as_u64().unwrap() >= 1);

    let (status, _) = call(
        &hub,
        "PUT",
        "/api/v1/process",
        Some(json!({ "checks": false, "notifications": false })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        hub.suppressed("web01/http", watcher_core::State::Critical)
            .as_deref(),
        Some("notifications are off")
    );
    let (_, schedule) = call(&hub, "GET", "/api/v1/scheduler", None).await;
    let http = schedule
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["service"] == "http")
        .unwrap();
    assert_eq!(http["paused"], true);
    assert_eq!(http["host"], "web01");
}

#[tokio::test]
async fn run_without_recording() {
    let hub = hub();
    let (status, run) = call(
        &hub,
        "POST",
        "/api/v1/run-check",
        Some(json!({ "command": "echo 'DISK OK | used=42%;80;90'", "shell": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{run}");
    assert_eq!(run["state"], "ok");
    assert_eq!(run["message"], "DISK OK");
    assert_eq!(run["metrics"][0]["name"], "used");
    assert_eq!(run["termination"], "check exited with code 0");

    // A configured service: its command runs, nothing is recorded.
    let (status, run) = call(&hub, "POST", "/api/v1/services/web01/http/run", None).await;
    assert_eq!(status, StatusCode::OK, "{run}");
    assert!(run["command"].as_str().unwrap().starts_with("check_http"));
    let (_, svc) = call(&hub, "GET", "/api/v1/services/web01/http", None).await;
    assert_eq!(svc["last_check"], Value::Null);
    let (status, _) = call(&hub, "POST", "/api/v1/services/db01/dump/run", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn downtime_changes_start_and_end() {
    let hub = hub();
    // The first call only takes note.
    assert!(hub.downtime_changes().await.is_empty());
    let (_, dt) = call(
        &hub,
        "POST",
        "/api/v1/actions/downtime",
        Some(json!({ "hosts": ["web01"], "services": ["db01/dump"], "duration": "1h", "comment": "patch" })),
    )
    .await;
    let changes = hub.downtime_changes().await;
    let got: Vec<(String, Option<String>, bool, String)> = changes
        .iter()
        .map(|c| (c.host.clone(), c.service.clone(), c.started, c.what.clone()))
        .collect();
    assert_eq!(
        got,
        [
            (
                "db01".into(),
                Some("dump".into()),
                true,
                "service db01/dump: patch".into()
            ),
            ("web01".into(), None, true, "host web01: patch".into()),
        ]
    );
    assert!(hub.downtime_changes().await.is_empty());
    for d in dt["downtimes"].as_array().unwrap() {
        let uri = format!("/api/v1/downtimes/{}", d["id"]);
        call(&hub, "DELETE", &uri, None).await;
    }
    let ended = hub.downtime_changes().await;
    assert_eq!(ended.len(), 2);
    assert!(ended.iter().all(|c| !c.started));
    assert_eq!(hub.downtime_suppressed("web01", None), None);
}
