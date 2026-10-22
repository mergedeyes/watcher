//! First-run setup in the browser: `watcher setup`, or `watcher run` with
//! a watcher.toml that doesn't exist yet. Serves the web UI's setup wizard,
//! unlocked with a one-time token from the log, writes the files like
//! `watcher init` and then hands over to the normal server.

use crate::init_cmd::{self, Answers, Plan};
use anyhow::{Context, Result};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;
use tracing::{info, warn};
use watcher_config::{Contact, Smtp};
use watcher_notify::{Notification, Notifier};
use watcher_store::Store;

#[derive(clap::Args)]
pub struct Args {
    /// Directory for watcher.toml and conf.d/.
    #[arg(default_value = ".")]
    dir: PathBuf,

    /// Address for the setup page, and the default for the server after
    /// setup. Use 0.0.0.0:8080 to set up a remote machine.
    #[arg(long, default_value = "127.0.0.1:8080", value_name = "ADDR")]
    listen: SocketAddr,
}

pub async fn command(args: Args) -> Result<std::process::ExitCode> {
    let config = run(&args.dir, args.listen).await?;
    crate::run::run(crate::run::Args::new(config)).await
}

struct Setup {
    dir: PathBuf,
    listen: SocketAddr,
    token: String,
    /// Fires with the written watcher.toml when setup is done.
    done: Mutex<Option<oneshot::Sender<PathBuf>>>,
}

/// Serves the setup wizard until it has written the configuration, and
/// returns the path of the new watcher.toml.
pub async fn run(dir: &Path, listen: SocketAddr) -> Result<PathBuf> {
    let token = token();
    let (done_tx, done_rx) = oneshot::channel();
    let setup = Arc::new(Setup {
        dir: dir.to_path_buf(),
        listen,
        token: token.clone(),
        done: Mutex::new(Some(done_tx)),
    });
    let router = Router::new()
        .route(
            "/api/v1/health",
            get(|| async { Json(json!({ "status": "setup" })) }),
        )
        .route("/api/v1/auth/status", get(status))
        .route("/api/v1/setup/unlock", post(unlock))
        .route("/api/v1/setup", get(defaults))
        .route("/api/v1/setup/database", post(database))
        .route("/api/v1/setup/notify-test", post(notify_test))
        .route("/api/v1/setup/plan", post(plan))
        .route("/api/v1/setup/apply", post(apply))
        .fallback(watcher_hub::serve_ui)
        .with_state(setup);
    let server = crate::run::serve(listen, router, None)?;
    info!("setup: open http://{listen}/setup");
    info!("setup token: {token}");
    if !listen.ip().is_loopback() {
        warn!("setup runs over plain HTTP on {listen}; use it on a trusted network");
    }
    let config = done_rx.await.context("setup ended")?;
    // Let the last response reach the browser.
    tokio::time::sleep(Duration::from_millis(300)).await;
    server.abort();
    let _ = server.await;
    info!("setup done: {}", config.display());
    Ok(config)
}

/// 20 random characters in groups, e.g. `k3m9-x2qa-...`.
fn token() -> String {
    const ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyz23456789";
    let mut bytes = [0u8; 20];
    getrandom::fill(&mut bytes).expect("system random number generator");
    let chars: Vec<char> = bytes
        .iter()
        .map(|b| ALPHABET[usize::from(*b) % ALPHABET.len()] as char)
        .collect();
    chars
        .chunks(5)
        .map(|c| c.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("-")
}

type Shared = State<Arc<Setup>>;

fn error(status: StatusCode, message: impl std::fmt::Display) -> Response {
    (status, Json(json!({ "error": message.to_string() }))).into_response()
}

/// Requests need the token in `X-Setup-Token`.
fn unlocked(setup: &Setup, headers: &HeaderMap) -> bool {
    let given = headers
        .get("x-setup-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    constant_time_eq(given.trim().as_bytes(), setup.token.as_bytes())
}

fn locked() -> Response {
    error(StatusCode::FORBIDDEN, "wrong setup token")
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Tells the web UI to show the wizard.
async fn status() -> Json<serde_json::Value> {
    Json(json!({ "required": false, "user": null, "users_exist": false, "setup": true }))
}

#[derive(Deserialize)]
struct UnlockBody {
    token: String,
}

async fn unlock(State(setup): Shared, Json(body): Json<UnlockBody>) -> Response {
    if constant_time_eq(body.token.trim().as_bytes(), setup.token.as_bytes()) {
        StatusCode::NO_CONTENT.into_response()
    } else {
        // Slows down guessing; the token has ~100 bits anyway.
        tokio::time::sleep(Duration::from_secs(1)).await;
        locked()
    }
}

/// Suggestions for the form.
async fn defaults(State(setup): Shared, headers: HeaderMap) -> Response {
    if !unlocked(&setup, &headers) {
        return locked();
    }
    let host = init_cmd::hostname();
    let existing: Vec<String> = ["watcher.toml", "conf.d"]
        .iter()
        .filter(|f| setup.dir.join(f).exists())
        .map(|f| f.to_string())
        .collect();
    Json(json!({
        "dir": setup.dir,
        "hostname": host,
        "listen": setup.listen.to_string(),
        "plugins": init_cmd::plugin_dir(),
        "existing": existing,
        "min_password": init_cmd::MIN_PASSWORD,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct DatabaseBody {
    url: String,
}

/// Connects, creates the schema and reports what was found.
async fn database(
    State(setup): Shared,
    headers: HeaderMap,
    Json(body): Json<DatabaseBody>,
) -> Response {
    if !unlocked(&setup, &headers) {
        return locked();
    }
    let connect = tokio::time::timeout(Duration::from_secs(15), Store::connect(&body.url)).await;
    match connect {
        Ok(Ok(store)) => {
            let users = store.user_count().await.unwrap_or(0);
            Json(json!({ "backend": store.backend().to_string(), "users": users })).into_response()
        }
        Ok(Err(err)) => error(StatusCode::UNPROCESSABLE_ENTITY, format!("{err:#}")),
        Err(_) => error(StatusCode::UNPROCESSABLE_ENTITY, "timeout after 15s"),
    }
}

#[derive(Deserialize)]
struct NotifyTestBody {
    smtp: Option<Smtp>,
    #[serde(default)]
    email: Vec<String>,
    webhook: Option<String>,
}

/// Sends a test notification with the entered settings.
async fn notify_test(
    State(setup): Shared,
    headers: HeaderMap,
    Json(body): Json<NotifyTestBody>,
) -> Response {
    if !unlocked(&setup, &headers) {
        return locked();
    }
    let notifier = match Notifier::new(body.smtp.as_ref(), None) {
        Ok(n) => n,
        Err(err) => return error(StatusCode::UNPROCESSABLE_ENTITY, err),
    };
    let contact = Contact {
        name: "admins".into(),
        email: body.email,
        webhook: body.webhook,
        origin: Default::default(),
    };
    let results: Vec<_> = notifier
        .deliver(&contact, &Notification::test("admins", None))
        .await
        .into_iter()
        .map(|(channel, result)| {
            json!({ "channel": channel, "error": result.err().map(|e| e.to_string()) })
        })
        .collect();
    Json(json!({ "results": results })).into_response()
}

/// The files the answers would write.
async fn plan(State(setup): Shared, headers: HeaderMap, Json(answers): Json<Answers>) -> Response {
    if !unlocked(&setup, &headers) {
        return locked();
    }
    match Plan::new(&answers, &setup.dir) {
        Ok(plan) => {
            let files: Vec<_> = plan
                .files
                .iter()
                .map(|f| {
                    // The SMTP password file holds nothing else.
                    let secret = f.path.file_name().is_some_and(|n| n == "smtp.pass");
                    json!({
                        "path": f.path,
                        "contents": if secret { "(password)\n" } else { f.contents.as_str() },
                        "exists": f.path.exists(),
                    })
                })
                .collect();
            Json(json!({ "files": files })).into_response()
        }
        Err(err) => error(StatusCode::UNPROCESSABLE_ENTITY, format!("{err:#}")),
    }
}

/// Writes everything, then hands over to the normal server.
async fn apply(State(setup): Shared, headers: HeaderMap, Json(answers): Json<Answers>) -> Response {
    if !unlocked(&setup, &headers) {
        return locked();
    }
    let plan = match Plan::new(&answers, &setup.dir) {
        Ok(plan) => plan,
        Err(err) => return error(StatusCode::UNPROCESSABLE_ENTITY, format!("{err:#}")),
    };
    if let Err(err) = init_cmd::apply(&answers, &plan).await {
        return error(StatusCode::UNPROCESSABLE_ENTITY, format!("{err:#}"));
    }
    let config = setup.dir.join("watcher.toml");
    let listen = answers
        .listen
        .clone()
        .unwrap_or_else(|| "127.0.0.1:8080".into());
    let scheme = if answers.tls.is_some() {
        "https"
    } else {
        "http"
    };
    if let Some(done) = setup.done.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = done.send(config.clone());
    }
    Json(json!({ "config": config, "listen": listen, "scheme": scheme })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_grouped_and_random() {
        let a = token();
        assert_eq!(a.len(), 23);
        assert_eq!(a.split('-').count(), 4);
        assert_ne!(a, token());
        assert!(constant_time_eq(a.as_bytes(), a.clone().as_bytes()));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }
}
