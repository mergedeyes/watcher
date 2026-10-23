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
use axum::extract::{Extension, Path, Query, State};
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
use watcher_config::{CheckDirs, ConfigMode, Diagnostic, Loaded, Severity, diff};

/// Where the object files are and who may edit them.
pub struct ConfigFiles {
    pub dir: PathBuf,
    pub mode: ConfigMode,
    /// Asks the run loop to reload after a change was written.
    pub reload: mpsc::Sender<Reload>,
    /// Where check commands are looked up: for warnings and the script list.
    pub check_dirs: CheckDirs,
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

/// Adds warnings for commands that aren't found in the check directories.
fn with_missing_commands(mut loaded: Loaded, check_dirs: &CheckDirs) -> Loaded {
    let missing = check_dirs.missing_commands(&loaded.config);
    loaded.warnings.extend(missing);
    loaded
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
    let check_dirs = files.check_dirs.clone();
    let (list, loaded) = blocking(move || {
        let list = files::list(&dir);
        let loaded = watcher_config::load_dir(&dir).map(|l| with_missing_commands(l, &check_dirs));
        (list, loaded)
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

/// The check directories and the executables in them, for command
/// suggestions.
pub async fn scripts(State(hub): Shared, user: User) -> Result<Response, ApiError> {
    admin(&hub, &user)?;
    let check_dirs = files(&hub)?.check_dirs.clone();
    let scripts = blocking(move || check_dirs.scripts()).await?;
    let dirs = files(&hub)?.check_dirs.dirs();
    Ok(Json(json!({ "dirs": dirs, "scripts": scripts })).into_response())
}

/// The Config Tree of an object in the running configuration (§4.6):
/// where it and its values come from, and where it is used.
pub async fn tree(
    State(hub): Shared,
    user: User,
    Path((kind, name)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    admin(&hub, &user)?;
    let config = hub.config();
    let base = hub.config_files.get().map(|f| f.dir.clone());
    let builder = watcher_config::tree::Builder::new(&config, base.as_deref());
    match builder.tree(&kind, &name) {
        Some(tree) => Ok(Json(tree).into_response()),
        None => Err(ApiError::NotFound(format!("no {kind} {name:?}"))),
    }
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
#[derive(Deserialize)]
pub struct ObjectsQuery {
    /// Only objects of this kind.
    kind: Option<Kind>,
}

pub async fn objects(
    State(hub): Shared,
    user: User,
    Query(q): Query<ObjectsQuery>,
) -> Result<Response, ApiError> {
    admin(&hub, &user)?;
    let files = files(&hub)?;
    let dir = files.dir.clone();
    let (mut objects, broken) = blocking(move || edit::objects(&dir))
        .await?
        .map_err(|e| ApiError::Internal(format!("cannot read the object files: {e}")))?;
    if let Some(kind) = q.kind {
        objects.retain(|o| o.kind == kind);
    }
    Ok(
        Json(json!({ "objects": objects, "diagnostics": problems(files, &broken) }))
            .into_response(),
    )
}

/// One object as written, with its file and line.
pub async fn object(
    State(hub): Shared,
    user: User,
    Path((kind, name)): Path<(Kind, String)>,
) -> Result<Response, ApiError> {
    admin(&hub, &user)?;
    let files = files(&hub)?;
    let dir = files.dir.clone();
    let (objects, _) = blocking(move || edit::objects(&dir))
        .await?
        .map_err(|e| ApiError::Internal(format!("cannot read the object files: {e}")))?;
    objects
        .into_iter()
        .find(|o| o.kind == kind && o.name == name)
        .map(|o| Json(o).into_response())
        .ok_or_else(|| ApiError::NotFound(format!("{} {name:?} not found", kind.key())))
}

#[derive(Deserialize)]
pub struct PutQuery {
    /// For a new object: the file it goes to (default: the kind's UI file).
    path: Option<String>,
}

/// Creates or replaces an object and applies it in one step: the body is
/// its fields as written (`name` comes from the path). 422 with the
/// diagnostics if the result has errors; nothing is written then.
pub async fn put_object(
    State(hub): Shared,
    user: User,
    Path((kind, name)): Path<(Kind, String)>,
    Query(q): Query<PutQuery>,
    Json(fields): Json<serde_json::Map<String, serde_json::Value>>,
) -> Result<Response, ApiError> {
    let by = admin(&hub, &user)?;
    let files = files(&hub)?;
    writable(files)?;
    let mut fields = toml::Table::deserialize(serde_json::Value::Object(fields))
        .map_err(|e| ApiError::BadRequest(format!("fields: {e}")))?;
    fields.insert("name".into(), toml::Value::String(name.clone()));
    let dir = files.dir.clone();
    let lookup = dir.clone();
    let (objects, _) = blocking(move || edit::objects(&lookup))
        .await?
        .map_err(|e| ApiError::Internal(format!("cannot read the object files: {e}")))?;
    let exists = objects.iter().any(|o| o.kind == kind && o.name == name);
    let e = edit::Edit {
        kind,
        name: exists.then_some(name),
        fields: Some(fields),
        path: q.path,
    };
    match blocking(move || edit::edit_many(&dir, &[e])).await? {
        Ok(changes) => apply_changes(&hub, files, by, changes).await,
        Err(err) => edit_error(files, err),
    }
}

/// Deletes an object and applies it in one step.
pub async fn delete_object(
    State(hub): Shared,
    user: User,
    Path((kind, name)): Path<(Kind, String)>,
) -> Result<Response, ApiError> {
    let by = admin(&hub, &user)?;
    let files = files(&hub)?;
    writable(files)?;
    let dir = files.dir.clone();
    let e = edit::Edit {
        kind,
        name: Some(name),
        fields: None,
        path: None,
    };
    match blocking(move || edit::edit_many(&dir, &[e])).await? {
        Ok(changes) => apply_changes(&hub, files, by, changes).await,
        Err(err) => edit_error(files, err),
    }
}

/// Reads the object files again, e.g. after editing them on the server.
pub async fn reload(State(hub): Shared, user: User) -> Result<Response, ApiError> {
    let by = admin(&hub, &user)?;
    let files = files(&hub)?;
    let (reply, rx) = oneshot::channel();
    let unavailable = || ApiError::Unavailable("the server is not reloading".into());
    files
        .reload
        .send(Reload { by, reply })
        .await
        .map_err(|_| unavailable())?;
    match rx.await.map_err(|_| unavailable())? {
        Ok(()) => Ok(Json(json!({ "reloaded": true })).into_response()),
        Err(err) => Err(ApiError::BadRequest(format!("not reloaded: {err}"))),
    }
}

#[derive(Deserialize)]
pub struct EditBody {
    kind: Kind,
    /// The object to change; absent to create one.
    name: Option<String>,
    /// The object's fields as written (including `name`); absent to delete.
    fields: Option<serde_json::Map<String, serde_json::Value>>,
    /// For a new object: the file it goes to (default: the kind's UI file).
    path: Option<String>,
}

impl EditBody {
    fn into_edit(self) -> Result<edit::Edit, ApiError> {
        let fields = self
            .fields
            .map(|f| {
                toml::Table::deserialize(serde_json::Value::Object(f))
                    .map_err(|e| ApiError::BadRequest(format!("fields: {e}")))
            })
            .transpose()?;
        Ok(edit::Edit {
            kind: self.kind,
            name: self.name,
            fields,
            path: self.path,
        })
    }
}

#[derive(Deserialize)]
pub struct EditManyBody {
    edits: Vec<EditBody>,
}

fn edit_error(files: &ConfigFiles, err: EditError) -> Result<Response, ApiError> {
    match err {
        EditError::File(err) => Ok(file_error(files, err)),
        err @ EditError::NotFound(..) => Err(ApiError::NotFound(err.to_string())),
        err @ EditError::Exists(..) => Err(ApiError::Conflict(err.to_string())),
        err => Err(ApiError::BadRequest(err.to_string())),
    }
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
    let e = body.into_edit()?;
    let dir = files.dir.clone();
    match blocking(move || edit::edit_many(&dir, &[e])).await? {
        Ok(mut changes) => Ok(Json(changes.remove(0)).into_response()),
        Err(err) => edit_error(files, err),
    }
}

/// The file changes for several objects at once, e.g. clones: edits of the
/// same file build on each other, giving one change per file.
pub async fn edit_many(
    State(hub): Shared,
    user: User,
    Json(body): Json<EditManyBody>,
) -> Result<Response, ApiError> {
    admin(&hub, &user)?;
    let files = files(&hub)?;
    writable(files)?;
    if body.edits.is_empty() || body.edits.len() > 1000 {
        return Err(ApiError::BadRequest("1 to 1000 edits".into()));
    }
    let edits = body
        .edits
        .into_iter()
        .map(EditBody::into_edit)
        .collect::<Result<Vec<_>, _>>()?;
    let dir = files.dir.clone();
    match blocking(move || edit::edit_many(&dir, &edits)).await? {
        Ok(changes) => Ok(Json(json!({ "changes": changes })).into_response()),
        Err(err) => edit_error(files, err),
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
    let check_dirs = files.check_dirs.clone();
    let changes = body.changes;
    let result = blocking(move || {
        files::check_base(&dir, &changes)?;
        files::preview(&dir, &changes).map(|l| with_missing_commands(l, &check_dirs))
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
    apply_changes(&hub, files, by, body.changes).await
}

/// Writes changes and reloads; see [`apply`].
async fn apply_changes(
    hub: &Hub,
    files: &ConfigFiles,
    by: String,
    changes: Vec<Change>,
) -> Result<Response, ApiError> {
    let _guard = hub.config_lock.lock().await;
    let dir = files.dir.clone();
    let paths: Vec<String> = changes.iter().map(|c| c.path.clone()).collect();
    let check_dirs = files.check_dirs.clone();
    let applied = blocking(move || {
        files::apply(&dir, &changes).map(|l| with_missing_commands(l, &check_dirs))
    });
    let loaded = match applied.await? {
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
