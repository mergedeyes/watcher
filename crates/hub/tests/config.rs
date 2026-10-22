//! Config editing over the API, with a stand-in for the run loop's reload.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tower::ServiceExt;
use watcher_config::{ConfigMode, load_dir};
use watcher_hub::{AuthSettings, ConfigFiles, Hub, Reload};

const HOSTS: &str = r#"# Web servers
[[host]]
name = "web01"   # primary
tags = ["web"]

[[check]]
name = "http"
apply_to = '"web" in host.tags'
command = "check_http -H $host.address$"
"#;

struct Setup {
    hub: Arc<Hub>,
    dir: PathBuf,
    reloads: mpsc::UnboundedReceiver<String>,
}

fn setup(name: &str, mode: ConfigMode) -> Setup {
    let dir =
        std::env::temp_dir().join(format!("watcher-hub-config-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("hosts.toml"), HOSTS).unwrap();
    let config = Arc::new(load_dir(&dir).unwrap().config);
    let (control, _control_rx) = mpsc::channel(8);
    let hub = Hub::new(config, HashMap::new(), control, None, AuthSettings::OFF);

    // Reload like the run loop does, and report who asked.
    let (reload_tx, mut reload_rx) = mpsc::channel::<Reload>(4);
    let (seen_tx, reloads) = mpsc::unbounded_channel();
    let (h, d) = (hub.clone(), dir.clone());
    tokio::spawn(async move {
        while let Some(request) = reload_rx.recv().await {
            h.set_config(Arc::new(load_dir(&d).unwrap().config));
            let _ = seen_tx.send(request.by);
            let _ = request.reply.send(Ok(()));
        }
    });
    hub.enable_config_editing(ConfigFiles {
        dir: dir.clone(),
        mode,
        reload: reload_tx,
    });
    Setup { hub, dir, reloads }
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
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn lists_files_and_objects() {
    let s = setup("list", ConfigMode::Hybrid);
    let (status, overview) = call(&s.hub, "GET", "/api/v1/config", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(overview["mode"], "hybrid");
    assert_eq!(overview["writable"], true);
    assert_eq!(overview["files"][0]["path"], "hosts.toml");
    assert_eq!(overview["diagnostics"], json!([]));

    let (_, file) = call(&s.hub, "GET", "/api/v1/config/files/hosts.toml", None).await;
    assert_eq!(file["text"], HOSTS);
    assert_eq!(file["hash"], overview["files"][0]["hash"]);
    let (status, _) = call(&s.hub, "GET", "/api/v1/config/files/../etc.toml", None).await;
    assert!(status.is_client_error());

    let (_, objects) = call(&s.hub, "GET", "/api/v1/config/objects", None).await;
    let host = &objects["objects"][0];
    assert_eq!(host["kind"], "host");
    assert_eq!(host["name"], "web01");
    assert_eq!(host["line"], 2);
    assert_eq!(host["fields"]["tags"], json!(["web"]));
}

#[tokio::test]
async fn create_preview_apply() {
    let mut s = setup("apply", ConfigMode::Hybrid);
    let (status, change) = call(
        &s.hub,
        "POST",
        "/api/v1/config/edit",
        Some(json!({ "kind": "host", "fields": { "name": "web02", "tags": ["web"] } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{change}");
    assert_eq!(change["path"], "ui/hosts.toml");
    assert_eq!(change["base"], Value::Null);

    let body = json!({ "changes": [change] });
    let (_, preview) = call(&s.hub, "POST", "/api/v1/config/preview", Some(body.clone())).await;
    assert_eq!(preview["valid"], true);
    assert_eq!(preview["changes"]["hosts"]["added"], json!(["web02"]));
    assert_eq!(
        preview["changes"]["services"]["added"],
        json!(["web02/http"])
    );
    assert!(!s.dir.join("ui/hosts.toml").exists());

    let (status, applied) = call(&s.hub, "POST", "/api/v1/config/apply", Some(body.clone())).await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert_eq!(applied["changes"]["hosts"]["added"], json!(["web02"]));
    assert_eq!(s.reloads.recv().await.unwrap(), "local user");
    assert_eq!(s.hub.config().services.len(), 2);
    assert!(
        std::fs::read_to_string(s.dir.join("ui/hosts.toml"))
            .unwrap()
            .contains("web02")
    );

    // The same change again: the file exists now.
    let (status, conflict) = call(&s.hub, "POST", "/api/v1/config/apply", Some(body)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(conflict["path"], "ui/hosts.toml");
    assert!(
        conflict["current"]["text"]
            .as_str()
            .unwrap()
            .contains("web02")
    );
}

#[tokio::test]
async fn update_keeps_comments() {
    let mut s = setup("update", ConfigMode::Hybrid);
    let (_, change) = call(
        &s.hub,
        "POST",
        "/api/v1/config/edit",
        Some(json!({
            "kind": "host",
            "name": "web01",
            "fields": { "name": "web01", "address": "10.0.0.5", "tags": ["web"] }
        })),
    )
    .await;
    assert_eq!(change["path"], "hosts.toml");
    let (status, applied) = call(
        &s.hub,
        "POST",
        "/api/v1/config/apply",
        Some(json!({ "changes": [change] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert_eq!(applied["changes"]["hosts"]["changed"], json!(["web01"]));
    s.reloads.recv().await.unwrap();
    let text = std::fs::read_to_string(s.dir.join("hosts.toml")).unwrap();
    assert!(text.starts_with("# Web servers\n[[host]]\nname = \"web01\"   # primary\n"));
    assert!(text.contains("address = \"10.0.0.5\""));
}

#[tokio::test]
async fn invalid_changes_are_not_written() {
    let s = setup("invalid", ConfigMode::Hybrid);
    let (_, file) = call(&s.hub, "GET", "/api/v1/config/files/hosts.toml", None).await;
    let broken = HOSTS.replace("check_http", "check_http $host.nope$");
    let body =
        json!({ "changes": [{ "path": "hosts.toml", "base": file["hash"], "text": broken }] });

    let (_, preview) = call(&s.hub, "POST", "/api/v1/config/preview", Some(body.clone())).await;
    assert_eq!(preview["valid"], false);
    let problem = &preview["diagnostics"][0];
    assert_eq!(problem["path"], "hosts.toml");
    assert_eq!(problem["line"], 6);
    assert_eq!(problem["severity"], "error");

    let (status, _) = call(&s.hub, "POST", "/api/v1/config/apply", Some(body)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        std::fs::read_to_string(s.dir.join("hosts.toml")).unwrap(),
        HOSTS
    );

    // Deleting the host would leave the check without hosts: a warning only.
    let (_, change) = call(
        &s.hub,
        "POST",
        "/api/v1/config/edit",
        Some(json!({ "kind": "host", "name": "web01" })),
    )
    .await;
    let (_, preview) = call(
        &s.hub,
        "POST",
        "/api/v1/config/preview",
        Some(json!({ "changes": [change] })),
    )
    .await;
    assert_eq!(preview["valid"], true);
    assert_eq!(
        preview["changes"]["services"]["removed"],
        json!(["web01/http"])
    );
    assert_eq!(preview["diagnostics"][0]["severity"], "warning");
}

#[tokio::test]
async fn files_mode_is_read_only() {
    let s = setup("readonly", ConfigMode::Files);
    let (_, overview) = call(&s.hub, "GET", "/api/v1/config", None).await;
    assert_eq!(overview["writable"], false);
    let (status, _) = call(
        &s.hub,
        "POST",
        "/api/v1/config/edit",
        Some(json!({ "kind": "host", "name": "web01" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &s.hub,
        "POST",
        "/api/v1/config/apply",
        Some(json!({ "changes": [] })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
