//! Authentication against a real database; runs when
//! `WATCHER_TEST_POSTGRES_URL` is set (see compose.yaml).

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::{AssertSqlSafe, Connection, PgConnection};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tower::ServiceExt;
use watcher_config::{CheckDirs, Config, ConfigMode};
use watcher_hub::auth::{hash_password, new_api_token};
use watcher_hub::{AuthSettings, ConfigFiles, Hub};
use watcher_store::Store;

/// Creates a database for test `test`; returns its URL and the admin URL.
async fn database(test: &str) -> Option<(String, String)> {
    let admin = std::env::var("WATCHER_TEST_POSTGRES_URL").ok()?;
    let name = format!("watcher_hub_auth_{test}_{}", std::process::id());
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
    Some((format!("{base}/{name}"), admin))
}

struct Client {
    hub: Arc<Hub>,
    cookie: Option<String>,
}

impl Client {
    async fn call(
        &mut self,
        method: &str,
        uri: &str,
        body: Option<Value>,
        extra: &[(&str, &str)],
    ) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::HOST, "watcher.test");
        if let Some(cookie) = &self.cookie {
            req = req.header(header::COOKIE, format!("watcher_session={cookie}"));
        }
        for (k, v) in extra {
            req = req.header(*k, *v);
        }
        let body = match body {
            Some(json) => {
                req = req.header(header::CONTENT_TYPE, "application/json");
                Body::from(json.to_string())
            }
            None => Body::empty(),
        };
        let resp = self
            .hub
            .clone()
            .router()
            .oneshot(req.body(body).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        if let Some(set) = resp.headers().get(header::SET_COOKIE) {
            let value = set.to_str().unwrap();
            let token = value
                .split(';')
                .next()
                .unwrap()
                .trim_start_matches("watcher_session=");
            self.cookie = (!token.is_empty()).then(|| token.to_string());
            assert!(
                value.contains("HttpOnly") && value.contains("SameSite=Strict"),
                "{value}"
            );
        }
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
}

#[tokio::test]
async fn login_sessions_and_tokens() {
    let Some((url, admin)) = database("login").await else {
        eprintln!("skipped: set WATCHER_TEST_POSTGRES_URL");
        return;
    };
    let store = Arc::new(Store::connect(&url).await.unwrap());
    store
        .create_user(
            "Alice",
            &hash_password("correct horse battery").unwrap(),
            true,
        )
        .await
        .unwrap();

    let (control, _rx) = mpsc::channel(1);
    let auth = AuthSettings {
        required: true,
        secure_cookies: false,
    };
    let hub = Hub::new(
        Arc::new(Config::default()),
        HashMap::new(),
        control,
        Some(store.clone()),
        auth,
    );
    let mut c = Client { hub, cookie: None };

    // Public: health, auth status, the UI. Everything else needs a login.
    assert_eq!(
        c.call("GET", "/api/v1/health", None, &[]).await.0,
        StatusCode::OK
    );
    assert_eq!(c.call("GET", "/", None, &[]).await.0, StatusCode::OK);
    let (status, err) = c.call("GET", "/api/v1/services", None, &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(err["error"].as_str().unwrap().contains("Bearer"));
    let (_, st) = c.call("GET", "/api/v1/auth/status", None, &[]).await;
    assert_eq!(
        (
            st["required"].clone(),
            st["user"].clone(),
            st["users_exist"].clone()
        ),
        (json!(true), Value::Null, json!(true))
    );

    // Wrong password, then right (usernames ignore case).
    let wrong = json!({"username": "alice", "password": "nope"});
    assert_eq!(
        c.call("POST", "/api/v1/auth/login", Some(wrong.clone()), &[])
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, body) = c
        .call(
            "POST",
            "/api/v1/auth/login",
            Some(json!({"username": "alice", "password": "correct horse battery"})),
            &[],
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["user"]["username"], "Alice");
    assert!(body["user"].get("password_hash").is_none());
    assert!(c.cookie.is_some());

    assert_eq!(
        c.call("GET", "/api/v1/services", None, &[]).await.0,
        StatusCode::OK
    );
    let (_, st) = c.call("GET", "/api/v1/auth/status", None, &[]).await;
    assert_eq!(st["user"]["username"], "Alice");

    // Tokens: created with the session, refused cross-site.
    let (status, _) = c
        .call(
            "POST",
            "/api/v1/tokens",
            Some(json!({"name": "cron"})),
            &[("origin", "https://evil.example")],
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, created) = c
        .call(
            "POST",
            "/api/v1/tokens",
            Some(json!({"name": "cron"})),
            &[("origin", "http://watcher.test")],
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let token = created["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("wt_"));
    let (status, _) = c
        .call("POST", "/api/v1/tokens", Some(json!({"name": "cron"})), &[])
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Config editing is for admins.
    let dir = std::env::temp_dir().join(format!("watcher-hub-auth-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (reload, _reloads) = mpsc::channel(1);
    c.hub.enable_config_editing(ConfigFiles {
        dir,
        mode: ConfigMode::Hybrid,
        reload,
        check_dirs: CheckDirs::default(),
    });
    assert_eq!(
        c.call("GET", "/api/v1/config", None, &[]).await.0,
        StatusCode::OK
    );
    let bob = store
        .create_user("bob", &hash_password("bob's password").unwrap(), false)
        .await
        .unwrap();
    let (bob_token, bob_hash) = new_api_token();
    store.create_token(bob.id, "bob", &bob_hash).await.unwrap();
    let bob_bearer = format!("Bearer {bob_token}");
    let (status, err) = c
        .call(
            "GET",
            "/api/v1/config",
            None,
            &[("authorization", &bob_bearer)],
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(err["error"], "admin rights required");

    // Logout ends the session; the token keeps working on its own.
    let session = c.cookie.clone();
    assert_eq!(
        c.call("POST", "/api/v1/auth/logout", None, &[]).await.0,
        StatusCode::NO_CONTENT
    );
    assert!(c.cookie.is_none());
    c.cookie = session;
    assert_eq!(
        c.call("GET", "/api/v1/services", None, &[]).await.0,
        StatusCode::UNAUTHORIZED
    );
    c.cookie = None;
    let bearer = format!("Bearer {token}");
    let (status, list) = c
        .call("GET", "/api/v1/tokens", None, &[("authorization", &bearer)])
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list[0]["name"], "cron");
    let id = list[0]["id"].as_i64().unwrap();
    let uri = format!("/api/v1/tokens/{id}");
    assert_eq!(
        c.call("DELETE", &uri, None, &[("authorization", &bearer)])
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        c.call("GET", "/api/v1/tokens", None, &[("authorization", &bearer)])
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );

    // Five wrong passwords lock the name for a minute, even the right one.
    for _ in 0..5 {
        c.call("POST", "/api/v1/auth/login", Some(wrong.clone()), &[])
            .await;
    }
    let right = json!({"username": "ALICE", "password": "correct horse battery"});
    assert_eq!(
        c.call("POST", "/api/v1/auth/login", Some(right), &[])
            .await
            .0,
        StatusCode::TOO_MANY_REQUESTS
    );

    drop(c);
    drop(store);
    let mut conn = PgConnection::connect(&admin).await.unwrap();
    let name = &url[url.rfind('/').unwrap() + 1..];
    sqlx::query(AssertSqlSafe(format!("DROP DATABASE {name} WITH (FORCE)")))
        .execute(&mut conn)
        .await
        .unwrap();
}

#[tokio::test]
async fn users_api_and_actions_with_a_token() {
    let Some((url, admin)) = database("users").await else {
        eprintln!("skipped: set WATCHER_TEST_POSTGRES_URL");
        return;
    };
    let store = Arc::new(Store::connect(&url).await.unwrap());
    let alice = store
        .create_user("alice", &hash_password("alice's password").unwrap(), true)
        .await
        .unwrap();
    let (token, hash) = new_api_token();
    store.create_token(alice.id, "ci", &hash).await.unwrap();
    let bearer = format!("Bearer {token}");
    let auth = [("authorization", bearer.as_str())];
    let (control, _rx) = mpsc::channel(8);
    let settings = AuthSettings {
        required: true,
        secure_cookies: false,
    };
    let hub = Hub::new(
        Arc::new(Config::default()),
        HashMap::new(),
        control,
        Some(store.clone()),
        settings,
    );
    let mut c = Client { hub, cookie: None };

    let (status, bob) = c
        .call(
            "POST",
            "/api/v1/users",
            Some(json!({ "username": "bob", "password": "bob's password" })),
            &auth,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{bob}");
    assert_eq!(bob["admin"], false);
    let (status, _) = c
        .call(
            "POST",
            "/api/v1/users",
            Some(json!({ "username": "carol", "password": "short" })),
            &auth,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, list) = c.call("GET", "/api/v1/users", None, &auth).await;
    assert_eq!(list.as_array().unwrap().len(), 2);

    // Bob may change his own password, nothing else.
    let bob_login = json!({ "username": "bob", "password": "bob's password" });
    assert_eq!(
        c.call("POST", "/api/v1/auth/login", Some(bob_login), &[])
            .await
            .0,
        StatusCode::OK
    );
    let (status, _) = c
        .call(
            "PUT",
            "/api/v1/users/bob",
            Some(json!({ "password": "bob's new password" })),
            &[("origin", "http://watcher.test")],
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    // A new password ends all sessions, this one too.
    let bob_login = json!({ "username": "bob", "password": "bob's new password" });
    assert_eq!(
        c.call("GET", "/api/v1/services", None, &[]).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        c.call("POST", "/api/v1/auth/login", Some(bob_login), &[])
            .await
            .0,
        StatusCode::OK
    );
    let (status, _) = c
        .call(
            "PUT",
            "/api/v1/users/bob",
            Some(json!({ "admin": true })),
            &[("origin", "http://watcher.test")],
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    c.cookie = None;

    // Admin with the token: make bob admin, disable, delete.
    let (_, changed) = c
        .call(
            "PUT",
            "/api/v1/users/bob",
            Some(json!({ "admin": true, "disabled": true })),
            &auth,
        )
        .await;
    assert_eq!(
        (changed["admin"].clone(), changed["disabled"].clone()),
        (json!(true), json!(true))
    );
    assert_eq!(
        c.call("DELETE", "/api/v1/users/alice", None, &auth).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        c.call("DELETE", "/api/v1/users/bob", None, &auth).await.0,
        StatusCode::NO_CONTENT
    );

    // Process switches with the token, stored in the database.
    let (status, _) = c
        .call(
            "PUT",
            "/api/v1/process",
            Some(json!({ "notifications": false })),
            &auth,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        store.setting("notifications").await.unwrap(),
        Some(json!(false))
    );

    drop(c);
    drop(store);
    let mut conn = PgConnection::connect(&admin).await.unwrap();
    let name = &url[url.rfind('/').unwrap() + 1..];
    sqlx::query(AssertSqlSafe(format!("DROP DATABASE {name} WITH (FORCE)")))
        .execute(&mut conn)
        .await
        .unwrap();
}

#[tokio::test]
async fn ssh_secrets_and_host_keys() {
    use watcher_exec::ssh::{Credential, Pool, Vault};
    use watcher_hub::StoreVault;

    let Some((url, _)) = database("ssh").await else {
        eprintln!("skipped: set WATCHER_TEST_POSTGRES_URL");
        return;
    };
    let store = Arc::new(Store::connect(&url).await.unwrap());
    let data_dir = std::env::temp_dir().join(format!("watcher-hub-ssh-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data_dir);
    let vault = Arc::new(StoreVault::new(store.clone(), data_dir.clone()));
    let (control, _rx) = mpsc::channel(1);
    let hub = Hub::new(
        Arc::new(Config::default()),
        HashMap::new(),
        control,
        Some(store.clone()),
        AuthSettings::OFF,
    );
    hub.enable_ssh(Arc::new(Pool::new(vault.clone())), Some(vault.clone()));
    let mut c = Client { hub, cookie: None };

    // Without secret.key nothing can be read; storing a secret creates it.
    let err = vault.credential("monitor").await.unwrap_err();
    assert!(err.contains("no such secret"), "{err}");
    let new = json!({ "name": "monitor", "kind": "generate" });
    let (status, created) = c
        .call("POST", "/api/v1/ssh/secrets", Some(new.clone()), &[])
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let public_key = created["public_key"].as_str().unwrap();
    assert!(public_key.starts_with("ssh-ed25519 ") && public_key.ends_with(" watcher-monitor"));
    assert!(data_dir.join("secret.key").exists());
    let (status, _) = c.call("POST", "/api/v1/ssh/secrets", Some(new), &[]).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let missing = json!({ "name": "pw", "kind": "password" });
    let (status, _) = c
        .call("POST", "/api/v1/ssh/secrets", Some(missing), &[])
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let password = json!({ "name": "pw", "kind": "password", "password": "s3cret" });
    let (status, _) = c
        .call("POST", "/api/v1/ssh/secrets", Some(password), &[])
        .await;
    assert_eq!(status, StatusCode::CREATED);

    let (_, list) = c.call("GET", "/api/v1/ssh/secrets", None, &[]).await;
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert_eq!(list[0]["name"], "monitor");
    let fingerprint = list[0]["fingerprint"].as_str().unwrap();
    assert!(list[0].get("sealed").is_none());
    let key = vault.credential("monitor").await.unwrap();
    assert_eq!(key.fingerprint().as_deref(), Some(fingerprint));
    assert!(
        matches!(vault.credential("pw").await.unwrap(), Credential::Password(p) if p == "s3cret")
    );

    // Host keys: pinned on first use, a changed one waits for acceptance.
    let host_key = |comment: &str| match Credential::generate(comment).unwrap() {
        Credential::Key(k) => k.public_key().clone(),
        Credential::Password(_) => unreachable!(),
    };
    let (a, b) = (host_key("a"), host_key("b"));
    vault.check_host_key("db1", 22, &a).await.unwrap();
    let err = vault.check_host_key("db1", 22, &b).await.unwrap_err();
    assert!(
        err.contains("host key changed") && err.contains("watcher hostkey accept db1"),
        "{err}"
    );
    let (_, keys) = c.call("GET", "/api/v1/ssh/hostkeys", None, &[]).await;
    assert_eq!(keys[0]["address"], "db1");
    assert!(
        keys[0]["offered_fingerprint"]
            .as_str()
            .unwrap()
            .starts_with("SHA256:")
    );
    let (status, _) = c
        .call("POST", "/api/v1/ssh/hostkeys/db1/22/accept", None, &[])
        .await;
    assert_eq!(status, StatusCode::OK);
    vault.check_host_key("db1", 22, &b).await.unwrap();
    let (status, _) = c
        .call("POST", "/api/v1/ssh/hostkeys/db1/22/accept", None, &[])
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = c
        .call("DELETE", "/api/v1/ssh/hostkeys/db1/22", None, &[])
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    vault.check_host_key("db1", 22, &a).await.unwrap();

    let (status, _) = c.call("DELETE", "/api/v1/ssh/secrets/pw", None, &[]).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = c.call("DELETE", "/api/v1/ssh/secrets/pw", None, &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    std::fs::remove_dir_all(&data_dir).unwrap();
}

#[tokio::test]
async fn two_factor_login() {
    use watcher_exec::ssh::Pool;
    use watcher_hub::{StoreVault, totp};

    let Some((url, _)) = database("totp").await else {
        eprintln!("skipped: set WATCHER_TEST_POSTGRES_URL");
        return;
    };
    let store = Arc::new(Store::connect(&url).await.unwrap());
    store
        .create_user("carol", &hash_password("carol's password").unwrap(), false)
        .await
        .unwrap();
    let admin = store
        .create_user("root", &hash_password("root's password").unwrap(), true)
        .await
        .unwrap();
    let (admin_token, admin_hash) = new_api_token();
    store
        .create_token(admin.id, "admin", &admin_hash)
        .await
        .unwrap();
    let admin_bearer = format!("Bearer {admin_token}");

    let (control, _rx) = mpsc::channel(1);
    let auth = AuthSettings {
        required: true,
        secure_cookies: false,
    };
    let hub = Hub::new(
        Arc::new(Config::default()),
        HashMap::new(),
        control,
        Some(store.clone()),
        auth,
    );
    let mut c = Client { hub, cookie: None };
    let login = |code: Option<&str>| {
        let mut body = json!({"username": "carol", "password": "carol's password"});
        if let Some(code) = code {
            body["code"] = json!(code);
        }
        Some(body)
    };
    let (status, _) = c.call("POST", "/api/v1/auth/login", login(None), &[]).await;
    assert_eq!(status, StatusCode::OK);

    // Without watcher.toml there is no secret.key to seal the secret with.
    let (status, err) = c.call("POST", "/api/v1/auth/totp/setup", None, &[]).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{err}");
    let data_dir = std::env::temp_dir().join(format!("watcher-hub-totp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data_dir);
    let vault = Arc::new(StoreVault::new(store.clone(), data_dir.clone()));
    c.hub
        .enable_ssh(Arc::new(Pool::new(vault.clone())), Some(vault));

    // Setting up: a secret and its QR code, stored once a code confirms it.
    let (_, st) = c.call("GET", "/api/v1/auth/totp", None, &[]).await;
    assert_eq!(st, json!({"enabled": false, "recovery_codes": 0}));
    let (status, setup) = c.call("POST", "/api/v1/auth/totp/setup", None, &[]).await;
    assert_eq!(status, StatusCode::OK, "{setup}");
    assert!(data_dir.join("secret.key").exists());
    assert!(
        setup["uri"]
            .as_str()
            .unwrap()
            .starts_with("otpauth://totp/Watcher:carol?secret=")
    );
    assert!(setup["qr"].as_str().unwrap().contains("<svg"));
    let secret_text = setup["secret"].as_str().unwrap().to_string();
    let secret = totp::decode(&secret_text).unwrap();
    let now = totp::now();
    let code = totp::code(&secret, now);
    let wrong = format!("{:06}", (code.parse::<u32>().unwrap() + 1) % 1_000_000);
    let enable = |code: &str| Some(json!({"secret": secret_text, "code": code}));
    let (status, _) = c
        .call("PUT", "/api/v1/auth/totp", enable(&wrong), &[])
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, on) = c.call("PUT", "/api/v1/auth/totp", enable(&code), &[]).await;
    assert_eq!(status, StatusCode::OK, "{on}");
    let recovery: Vec<String> = serde_json::from_value(on["recovery_codes"].clone()).unwrap();
    assert_eq!(recovery.len(), 10);
    let (status, _) = c.call("PUT", "/api/v1/auth/totp", enable(&code), &[]).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (_, st) = c.call("GET", "/api/v1/auth/status", None, &[]).await;
    assert_eq!(st["user"]["totp"], true);

    // Logging in: the password first, then a code; each code works once.
    c.call("POST", "/api/v1/auth/logout", None, &[]).await;
    let (status, body) = c.call("POST", "/api/v1/auth/login", login(None), &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["totp_required"], true, "{body}");
    assert!(c.cookie.is_none());
    let (status, body) = c
        .call("POST", "/api/v1/auth/login", login(Some(&code)), &[])
        .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the code that turned it on"
    );
    assert_eq!(body["error"], "invalid two-factor code");
    let next = totp::code(&secret, now + 1);
    let spaced = format!("{} {}", &next[..3], &next[3..]);
    let (status, body) = c
        .call("POST", "/api/v1/auth/login", login(Some(&spaced)), &[])
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(c.cookie.is_some());

    c.call("POST", "/api/v1/auth/logout", None, &[]).await;
    let upper = recovery[0].to_uppercase();
    let (status, _) = c
        .call("POST", "/api/v1/auth/login", login(Some(&upper)), &[])
        .await;
    assert_eq!(status, StatusCode::OK);
    c.call("POST", "/api/v1/auth/logout", None, &[]).await;
    let (status, _) = c
        .call("POST", "/api/v1/auth/login", login(Some(&recovery[0])), &[])
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "a used recovery code");
    let (status, _) = c
        .call("POST", "/api/v1/auth/login", login(Some(&recovery[1])), &[])
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, st) = c.call("GET", "/api/v1/auth/totp", None, &[]).await;
    assert_eq!(st, json!({"enabled": true, "recovery_codes": 8}));

    // New recovery codes replace the old ones; both need a code.
    let code_body = |code: &str| Some(json!({ "code": code }));
    let (status, _) = c
        .call("POST", "/api/v1/auth/totp/recovery", code_body("nope"), &[])
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, fresh) = c
        .call(
            "POST",
            "/api/v1/auth/totp/recovery",
            code_body(&recovery[2]),
            &[],
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{fresh}");
    let fresh: Vec<String> = serde_json::from_value(fresh["recovery_codes"].clone()).unwrap();
    let (status, _) = c
        .call("DELETE", "/api/v1/auth/totp", code_body(&recovery[3]), &[])
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "an old recovery code");

    // Only admins turn off another user's two-factor, and nobody turns it on.
    let (status, _) = c
        .call(
            "PUT",
            "/api/v1/users/carol",
            Some(json!({"totp": false})),
            &[],
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = c
        .call(
            "PUT",
            "/api/v1/users/carol",
            Some(json!({"totp": true})),
            &[("authorization", &admin_bearer)],
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Turning it off with a code: the password alone is enough again.
    let (status, _) = c
        .call("DELETE", "/api/v1/auth/totp", code_body(&fresh[0]), &[])
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, st) = c.call("GET", "/api/v1/auth/totp", None, &[]).await;
    assert_eq!(st, json!({"enabled": false, "recovery_codes": 0}));
    c.call("POST", "/api/v1/auth/logout", None, &[]).await;
    let (status, _) = c.call("POST", "/api/v1/auth/login", login(None), &[]).await;
    assert_eq!(status, StatusCode::OK);

    // A lost phone: an admin turns it off.
    let (_, setup) = c.call("POST", "/api/v1/auth/totp/setup", None, &[]).await;
    let secret_text = setup["secret"].as_str().unwrap().to_string();
    let code = totp::code(&totp::decode(&secret_text).unwrap(), totp::now());
    let (status, _) = c
        .call(
            "PUT",
            "/api/v1/auth/totp",
            Some(json!({"secret": secret_text, "code": code})),
            &[],
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, user) = c
        .call(
            "PUT",
            "/api/v1/users/carol",
            Some(json!({"totp": false})),
            &[("authorization", &admin_bearer)],
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{user}");
    assert_eq!(user["totp"], false);
    std::fs::remove_dir_all(&data_dir).unwrap();
}
