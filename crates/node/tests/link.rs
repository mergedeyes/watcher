//! A node joining a hub and keeping its session, against a real database;
//! runs when `WATCHER_TEST_POSTGRES_URL` is set (see compose.yaml).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::{AssertSqlSafe, Connection, PgConnection};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tower::ServiceExt;
use watcher_config::Config;
use watcher_hub::ca::Ca;
use watcher_hub::{AuthSettings, Hub, NodeLink, new_join_token, serve_node_link};
use watcher_proto::{JoinToken, fingerprint};
use watcher_store::Store;

async fn database(test: &str) -> Option<String> {
    let admin = std::env::var("WATCHER_TEST_POSTGRES_URL").ok()?;
    let name = format!("watcher_node_{test}_{}", std::process::id());
    let mut conn = PgConnection::connect(&admin).await.unwrap();
    sqlx::query(AssertSqlSafe(format!(
        "DROP DATABASE IF EXISTS {name} WITH (FORCE)"
    )))
    .execute(&mut conn)
    .await
    .unwrap();
    sqlx::query(AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .execute(&mut conn)
        .await
        .unwrap();
    let base = &admin[..admin.rfind('/').unwrap()];
    Some(format!("{base}/{name}"))
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("watcher-node-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

async fn call(hub: &Arc<Hub>, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    let request = match body {
        Some(json) => request.body(Body::from(json.to_string())),
        None => request.body(Body::empty()),
    }
    .unwrap();
    let response = hub.clone().router().oneshot(request).await.unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// The node `name` from `GET /nodes`.
async fn node(hub: &Arc<Hub>, name: &str) -> Option<Value> {
    let (status, list) = call(hub, "GET", "/api/v1/nodes", None).await;
    assert_eq!(status, StatusCode::OK);
    list.as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"] == name)
        .cloned()
}

/// Waits until node `name` is in `state` (or gone, for `None`).
async fn wait_for(hub: &Arc<Hub>, name: &str, state: Option<&str>) -> Option<Value> {
    for _ in 0..100 {
        let node = node(hub, name).await;
        if node.as_ref().map(|n| n["state"].as_str().unwrap()) == state {
            return node;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!(
        "node {name} never got to {state:?}: {:?}",
        node(hub, name).await
    );
}

#[tokio::test]
async fn join_connect_rejoin_remove() {
    let Some(url) = database("link").await else {
        eprintln!("skipped: set WATCHER_TEST_POSTGRES_URL");
        return;
    };
    let store = Arc::new(Store::connect(&url).await.unwrap());
    let hub_dir = temp_dir("hub");
    let node_dir = temp_dir("node");
    let ca = Arc::new(Ca::open_or_create(&hub_dir).unwrap());
    let (control, _rx) = mpsc::channel(1);
    let hub = Hub::new(
        Arc::new(Config::default()),
        HashMap::new(),
        control,
        Some(store.clone()),
        AuthSettings::OFF,
    );
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let hub_url = format!(
        "https://127.0.0.1:{}",
        listener.local_addr().unwrap().port()
    );
    hub.enable_nodes(NodeLink {
        ca: ca.clone(),
        url: hub_url.clone(),
    });
    serve_node_link(hub.clone(), listener).unwrap();

    // Added on the hub: a join token and the command for the node.
    let (status, added) = call(
        &hub,
        "POST",
        "/api/v1/nodes",
        Some(json!({ "name": "site-a" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{added}");
    let token: JoinToken = added["token"].as_str().unwrap().parse().unwrap();
    assert_eq!(token.ca, ca.fingerprint());
    assert_eq!(
        added["command"],
        format!("watcher node join {hub_url} --token {token}")
    );
    assert_eq!(
        wait_for(&hub, "site-a", Some("new")).await.unwrap()["hosts"],
        json!([])
    );
    let (status, _) = call(
        &hub,
        "POST",
        "/api/v1/nodes",
        Some(json!({ "name": "a b" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // A token that names another CA: the node doesn't trust the hub.
    let wrong = JoinToken {
        ca: fingerprint(b"another CA"),
        ..token.clone()
    };
    let err = watcher_node::join(&hub_url, &wrong, &node_dir)
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("not from the CA the join token names"));
    assert!(!watcher_node::joined(&node_dir));

    let joined = watcher_node::join(&hub_url, &token, &node_dir)
        .await
        .unwrap();
    assert_eq!(joined.name, "site-a");
    assert!(!joined.replaced);
    let key = node_dir.join("node/key.pem");
    let mode =
        std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&key).unwrap().permissions());
    assert_eq!(mode & 0o777, 0o600);
    // A token works once.
    let err = watcher_node::join(&hub_url, &token, &temp_dir("other"))
        .await
        .unwrap_err();
    assert!(
        format!("{err:#}").contains("unknown, used or expired"),
        "{err:#}"
    );

    let (stop, stopped) = oneshot::channel::<()>();
    let running = tokio::spawn({
        let (hub_url, node_dir) = (hub_url.clone(), node_dir.clone());
        async move {
            let shutdown = async {
                let _ = stopped.await;
            };
            watcher_node::run(&hub_url, &node_dir, "test-host", shutdown).await
        }
    });
    let connected = wait_for(&hub, "site-a", Some("connected")).await.unwrap();
    assert_eq!(connected["hostname"], "test-host");
    assert_eq!(connected["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(connected["address"], "127.0.0.1");
    assert!(connected["token_expires"].is_null());

    // Joining again (a reinstalled machine): the old certificate stops
    // working, and the running node picks up the new one.
    let old_cert = std::fs::read(node_dir.join("node/cert.pem")).unwrap();
    let (status, _) = call(
        &hub,
        "POST",
        "/api/v1/nodes",
        Some(json!({ "name": "site-a" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "joined nodes get a token only to join again"
    );
    let (again, _) = new_join_token(&store, &ca, "site-a", true).await.unwrap();
    let joined = watcher_node::join(&hub_url, &again, &node_dir)
        .await
        .unwrap();
    assert!(joined.replaced);
    assert_ne!(
        std::fs::read(node_dir.join("node/cert.pem")).unwrap(),
        old_cert
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    wait_for(&hub, "site-a", Some("connected")).await;

    // Removed: the connection ends and the node is refused from then on.
    let (status, _) = call(&hub, "DELETE", "/api/v1/nodes/site-a", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    wait_for(&hub, "site-a", None).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(node(&hub, "site-a").await.is_none());
    let (status, _) = call(&hub, "DELETE", "/api/v1/nodes/site-a", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let _ = stop.send(());
    running.await.unwrap().unwrap();
    for dir in [hub_dir, node_dir, temp_dir("other")] {
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[tokio::test]
async fn a_node_that_has_not_joined_does_not_run() {
    let dir = temp_dir("unjoined");
    let err = watcher_node::run("https://hub.example.com", &dir, "host", async {})
        .await
        .unwrap_err();
    assert!(err.to_string().contains("watcher node join"), "{err}");
}
