//! Discovery over the API: accepting guests as hosts, ignoring them, and
//! keeping the generated file in line with what discovery found. Needs
//! `WATCHER_TEST_POSTGRES_URL`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::{AssertSqlSafe, Connection, PgConnection};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tower::ServiceExt;
use watcher_config::{CheckDirs, ConfigMode, load_dir};
use watcher_hub::{AuthSettings, ConfigFiles, Hub, Reload};
use watcher_store::{FoundGuest, Store};

const CONFIG: &str = r#"
[[host]]
name = "pve1"
address = "192.0.2.10"
proxmox = { secret = "pve", discover = true }

# Already monitored by hand: discovery must not take the name.
[[host]]
name = "jellyfin"
address = "192.0.2.99"
"#;

async fn database() -> Option<String> {
    let admin = std::env::var("WATCHER_TEST_POSTGRES_URL").ok()?;
    let name = format!("watcher_hub_discovery_{}", std::process::id());
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

fn host<'c>(config: &'c watcher_config::Config, name: &str) -> Option<&'c watcher_config::Host> {
    config.hosts.iter().find(|h| h.name == name)
}

fn guest(vmid: u32, name: &str, kind: &str, node: &str, address: Option<&str>) -> FoundGuest {
    FoundGuest {
        vmid,
        kind: kind.into(),
        name: name.into(),
        node: node.into(),
        running: address.is_some(),
        address: address.map(String::from),
    }
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
async fn accept_ignore_and_follow_changes() {
    let Some(url) = database().await else {
        eprintln!("skipped: set WATCHER_TEST_POSTGRES_URL");
        return;
    };
    let store = Arc::new(Store::connect(&url).await.unwrap());
    let dir: PathBuf =
        std::env::temp_dir().join(format!("watcher-hub-discovery-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("proxmox.toml"), CONFIG).unwrap();
    let config = Arc::new(load_dir(&dir).unwrap().config);
    let (control, _control_rx) = mpsc::channel(8);
    let hub = Hub::new(
        config,
        HashMap::new(),
        control,
        Some(store.clone()),
        AuthSettings::OFF,
    );
    let (reload_tx, mut reload_rx) = mpsc::channel::<Reload>(4);
    let (h, d) = (hub.clone(), dir.clone());
    tokio::spawn(async move {
        while let Some(request) = reload_rx.recv().await {
            h.set_config(Arc::new(load_dir(&d).unwrap().config));
            let _ = request.reply.send(Ok(()));
        }
    });
    hub.enable_config_editing(ConfigFiles {
        dir: dir.clone(),
        mode: ConfigMode::Hybrid,
        reload: reload_tx,
        check_dirs: CheckDirs::new(vec![]),
    });

    // What a run found.
    let found = [
        guest(100, "jellyfin", "lxc", "pve", Some("192.0.2.71")),
        guest(101, "opnsense", "qemu", "pve", None),
        guest(102, "db", "qemu", "pve", Some("192.0.2.73")),
    ];
    store.sync_discovered("pve1", &found).await.unwrap();
    let (status, list) = call(&hub, "GET", "/api/v1/discovery", None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list[0]["name"], "pve1");
    assert_eq!(list[0]["url"], "https://192.0.2.10:8006");
    assert_eq!(list[0]["file"], "discovered/pve1.toml");
    assert_eq!(list[0]["guests"].as_array().unwrap().len(), 3);
    assert_eq!(list[0]["guests"][0]["host"], Value::Null);

    // Accepted: jellyfin is taken, so the host gets the VM ID.
    let (status, body) = call(
        &hub,
        "POST",
        "/api/v1/discovery/pve1/accept",
        Some(json!({ "vmids": [100, 101] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let file = std::fs::read_to_string(dir.join("discovered/pve1.toml")).unwrap();
    assert!(file.contains("name = \"jellyfin-100\""), "{file}");
    let config = hub.config();
    let ct = host(&config, "jellyfin-100").unwrap();
    assert_eq!(ct.address, "192.0.2.71");
    let p = ct.proxmox.as_ref().unwrap();
    assert_eq!((p.host.as_deref(), p.vmid), (Some("pve1"), Some(100)));
    assert_eq!(ct.host_check.as_deref(), Some("ping"));
    // The guest check asks the Proxmox VE host, with its login.
    assert_eq!(
        config
            .service("jellyfin-100/proxmox-guest")
            .unwrap()
            .command
            .as_ref()
            .unwrap()
            .to_string(),
        "proxmox-guest https://192.0.2.10:8006 vmid=100 secret=pve (built-in)"
    );
    // Without an address: the API decides whether it is UP.
    let vm = host(&config, "opnsense").unwrap();
    assert_eq!(vm.host_check.as_deref(), Some("proxmox-guest"));
    let (status, _) = call(
        &hub,
        "POST",
        "/api/v1/discovery/pve1/accept",
        Some(json!({ "vmids": [100] })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    let (status, _) = call(
        &hub,
        "POST",
        "/api/v1/discovery/pve1/ignore",
        Some(json!({ "vmids": [102] })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, list) = call(&hub, "GET", "/api/v1/discovery", None).await;
    let guests = list[0]["guests"].as_array().unwrap();
    assert_eq!(guests[0]["host"], "jellyfin-100");
    assert_eq!(guests[1]["host"], "opnsense");
    assert_eq!(guests[2]["ignored"], true);

    // Added by hand in the file: kept when discovery updates the host.
    let path = dir.join("discovered/pve1.toml");
    let text = std::fs::read_to_string(&path).unwrap();
    let text = text.replacen(
        r#"services = ["proxmox-guest"]"#,
        r#"services = ["proxmox-guest", "tls"]"#,
        1,
    );
    std::fs::write(&path, text).unwrap();

    // The next run: the container got a new address, the VM is gone.
    store
        .sync_discovered(
            "pve1",
            &[
                guest(100, "jellyfin", "lxc", "pve2", Some("192.0.2.81")),
                guest(102, "db", "qemu", "pve", Some("192.0.2.73")),
            ],
        )
        .await
        .unwrap();
    watcher_hub::discovery::update_hosts(&hub, "pve1")
        .await
        .unwrap();
    let config = hub.config();
    assert!(host(&config, "opnsense").is_none());
    assert_eq!(host(&config, "jellyfin-100").unwrap().address, "192.0.2.81");
    assert!(config.service("jellyfin-100/tls").is_some());
    // Nothing changed: the file stays as it is.
    let before = std::fs::read_to_string(dir.join("discovered/pve1.toml")).unwrap();
    watcher_hub::discovery::update_hosts(&hub, "pve1")
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("discovered/pve1.toml")).unwrap(),
        before
    );

    // Removed by hand: offered again.
    let (status, body) = call(
        &hub,
        "POST",
        "/api/v1/discovery/pve1/remove",
        Some(json!({ "vmids": [100] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(host(&hub.config(), "jellyfin-100").is_none());
    let (_, list) = call(&hub, "GET", "/api/v1/discovery", None).await;
    assert_eq!(list[0]["guests"][0]["host"], Value::Null);
    let (status, _) = call(&hub, "POST", "/api/v1/discovery/nope/run", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(&hub, "POST", "/api/v1/discovery/pve1/run", None).await;
    assert_eq!(status, StatusCode::ACCEPTED);
}
