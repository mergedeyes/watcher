//! Config editing over the API: read the object files, turn object edits
//! into file changes, preview them, and apply them. Admins only.
//!
//! Every change carries the hash of the file it was made on; a file that
//! changed in the meantime gives 409 with its current content, so the
//! client can show a diff instead of overwriting.

use crate::Hub;
use crate::api::ApiError;
use crate::auth::CurrentUser;
use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tracing::info;
use watcher_config::edit::{self, EditError, Kind};
use watcher_config::files::{self, Change};
use watcher_config::{ConfigMode, Diagnostic, Severity, diff};

/// Where the object files are and who may edit them.
pub struct ConfigFiles {
    pub dir: PathBuf,
    pub mode: ConfigMode,
    /// Asks the run loop to reload after a change was written.
    pub reload: mpsc::Sender<Reload>,
}

/// A request to reload the object files, answered with the result.
pub struct Reload {
    /// Who changed the files, for the log.
    pub by: String,
    pub reply: oneshot::Sender<Result<(), String>>,
}

type Shared = State<Arc<Hub>>;
type User = Option<Extension<CurrentUser>>;

/// Who is making the request: admins only, or anyone when login is off
/// (the server then only listens on loopback).
fn admin(hub: &Hub, user: &User) -> Result<String, ApiError> {
    if !hub.auth.required {
        return Ok("local user".into());
    }
    match user {
        Some(Extension(CurrentUser(u))) if u.admin => Ok(u.username.clone()),
        _ => Err(ApiError::Forbidden("admin rights required".into())),
    }
}

fn files(hub: &Hub) -> Result<&ConfigFiles, ApiError> {
    hub.config_files
        .get()
        .ok_or_else(|| ApiError::Unavailable("config editing is not available".into()))
}

fn writable(files: &ConfigFiles) -> Result<(), ApiError> {
    match files.mode {
        ConfigMode::Files => Err(ApiError::Forbidden(
            "the configuration is read-only here (config_mode = \"files\")".into(),
        )),
        _ => Ok(()),
    }
}

/// A diagnostic with its path relative to the object directory.
#[derive(Serialize)]
struct Problem {
    path: Option<String>,
    line: usize,
    severity: &'static str,
    message: String,
}

fn problems(files: &ConfigFiles, diagnostics: &[Diagnostic]) -> Vec<Problem> {
    diagnostics
        .iter()
        .map(|d| Problem {
            path: d.origin.as_ref().map(|o| {
                files::relative(&files.dir, &o.file)
                    .unwrap_or_else(|| o.file.to_string_lossy().into_owned())
            }),
            line: d.origin.as_ref().map_or(0, |o| o.line),
            severity: match d.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
            },
            message: d.message.clone(),
        })
        .collect()
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))
}

/// Errors with the details a client needs to recover.
fn file_error(files: &ConfigFiles, err: files::Error) -> Response {
    match err {
        files::Error::Path(_) => ApiError::BadRequest(err.to_string()).into_response(),
        files::Error::Conflict {
            ref path,
            ref current,
        } => (
            StatusCode::CONFLICT,
            Json(json!({ "error": err.to_string(), "path": path, "current": current })),
        )
            .into_response(),
        files::Error::Invalid(ref diagnostics) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "error": err.to_string(), "diagnostics": problems(files, diagnostics) })),
        )
            .into_response(),
        files::Error::Io { .. } => {
            tracing::error!("{err}");
            ApiError::Internal(err.to_string()).into_response()
        }
    }
}

/// The mode and the object files, with problems of the files as they are
/// on disk (the running configuration may be older if they have errors).
pub async fn overview(State(hub): Shared, user: User) -> Result<Response, ApiError> {
    admin(&hub, &user)?;
    let files = files(&hub)?;
    let dir = files.dir.clone();
    let (list, loaded) = blocking(move || {
        let list = files::list(&dir);
        (list, watcher_config::load_dir(&dir))
    })
    .await?;
    let list =
        list.map_err(|e| ApiError::Internal(format!("cannot read the object files: {e}")))?;
    let diagnostics = match loaded {
        Ok(loaded) => loaded.warnings,
        Err(diagnostics) => diagnostics,
    };
    let list: Vec<_> = list
        .into_iter()
        .map(|f| json!({ "path": f.path, "hash": f.hash, "size": f.text.len() }))
        .collect();
    Ok(Json(json!({
        "mode": files.mode.as_str(),
        "writable": files.mode != ConfigMode::Files,
        "dir": files.dir,
        "files": list,
        "diagnostics": problems(files, &diagnostics),
    }))
    .into_response())
}

pub async fn file(
    State(hub): Shared,
    user: User,
    Path(path): Path<String>,
) -> Result<Response, ApiError> {
    admin(&hub, &user)?;
    let files = files(&hub)?;
    let dir = files.dir.clone();
    let p = path.clone();
    match blocking(move || files::read(&dir, &p)).await? {
        Ok(Some(file)) => Ok(Json(file).into_response()),
        Ok(None) => Err(ApiError::NotFound(format!("no file {path:?}"))),
        Err(err) => Ok(file_error(files, err)),
    }
}

/// All objects as written, before templates apply.
pub async fn objects(State(hub): Shared, user: User) -> Result<Response, ApiError> {
    admin(&hub, &user)?;
    let files = files(&hub)?;
    let dir = files.dir.clone();
    let (objects, broken) = blocking(move || edit::objects(&dir))
        .await?
        .map_err(|e| ApiError::Internal(format!("cannot read the object files: {e}")))?;
    Ok(
        Json(json!({ "objects": objects, "diagnostics": problems(files, &broken) }))
            .into_response(),
    )
}

#[derive(Deserialize)]
pub struct EditBody {
    kind: Kind,
    /// The object to change; absent to create one.
    name: Option<String>,
    /// The object's fields as written (including `name`); absent to delete.
    fields: Option<serde_json::Map<String, serde_json::Value>>,
}

/// The file change for creating, updating or deleting one object. Nothing
/// is written; preview or apply the change.
pub async fn edit(
    State(hub): Shared,
    user: User,
    Json(body): Json<EditBody>,
) -> Result<Response, ApiError> {
    admin(&hub, &user)?;
    let files = files(&hub)?;
    writable(files)?;
    let fields = body
        .fields
        .map(|f| {
            toml::Table::deserialize(serde_json::Value::Object(f))
                .map_err(|e| ApiError::BadRequest(format!("fields: {e}")))
        })
        .transpose()?;
    let dir = files.dir.clone();
    let result =
        blocking(move || edit::edit(&dir, body.kind, body.name.as_deref(), fields.as_ref()))
            .await?;
    match result {
        Ok(change) => Ok(Json(change).into_response()),
        Err(EditError::File(err)) => Ok(file_error(files, err)),
        Err(err @ EditError::NotFound(..)) => Err(ApiError::NotFound(err.to_string())),
        Err(err @ EditError::Exists(..)) => Err(ApiError::Conflict(err.to_string())),
        Err(err) => Err(ApiError::BadRequest(err.to_string())),
    }
}

#[derive(Deserialize)]
pub struct ChangesBody {
    changes: Vec<Change>,
}

/// What `changes` would do: errors and warnings, and the hosts, services,
/// contacts and sinks they add, remove or change. 409 if a file changed
/// since it was read.
pub async fn preview(
    State(hub): Shared,
    user: User,
    Json(body): Json<ChangesBody>,
) -> Result<Response, ApiError> {
    admin(&hub, &user)?;
    let files = files(&hub)?;
    let dir = files.dir.clone();
    let changes = body.changes;
    let result = blocking(move || {
        files::check_base(&dir, &changes)?;
        files::preview(&dir, &changes)
    })
    .await?;
    let loaded = match result {
        Ok(loaded) => loaded,
        Err(files::Error::Invalid(diagnostics)) => {
            return Ok(Json(json!({
                "valid": false,
                "diagnostics": problems(files, &diagnostics),
            }))
            .into_response());
        }
        Err(err) => return Ok(file_error(files, err)),
    };
    let running = hub.config();
    Ok(Json(json!({
        "valid": true,
        "changes": diff::changes(&running, &loaded.config),
        "diagnostics": problems(files, &loaded.warnings),
    }))
    .into_response())
}

/// Writes `changes` if they are valid and were made on the current files,
/// then reloads. One apply at a time.
pub async fn apply(
    State(hub): Shared,
    user: User,
    Json(body): Json<ChangesBody>,
) -> Result<Response, ApiError> {
    let by = admin(&hub, &user)?;
    let files = files(&hub)?;
    writable(files)?;
    let _guard = hub.config_lock.lock().await;
    let dir = files.dir.clone();
    let changes = body.changes;
    let paths: Vec<String> = changes.iter().map(|c| c.path.clone()).collect();
    let loaded = match blocking(move || files::apply(&dir, &changes)).await? {
        Ok(loaded) => loaded,
        Err(err) => return Ok(file_error(files, err)),
    };
    info!("config changed by {by}: {}", paths.join(", "));
    let summary = diff::changes(&hub.config(), &loaded.config);

    let (reply, rx) = oneshot::channel();
    let unavailable = || ApiError::Unavailable("saved, but the server is not reloading".into());
    files
        .reload
        .send(Reload { by, reply })
        .await
        .map_err(|_| unavailable())?;
    if let Err(err) = rx.await.map_err(|_| unavailable())? {
        return Err(ApiError::Internal(format!(
            "saved, but the reload failed: {err}"
        )));
    }
    Ok(Json(json!({
        "changes": summary,
        "diagnostics": problems(files, &loaded.warnings),
    }))
    .into_response())
}
