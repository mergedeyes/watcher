//! Discovery: finds the VMs and containers of Proxmox VE hosts with
//! `proxmox = { discover = true }` and offers them as hosts. What a run
//! finds is kept in the database; an admin accepts a guest, which writes
//! it as a host to `discovered/<Proxmox VE host>.toml`, with the service
//! `proxmox-guest`. That file is kept up to date here: hosts of guests
//! deleted in Proxmox VE are removed, and a changed address is written.
//! Admins only.

use crate::Hub;
use crate::api::ApiError;
use crate::auth::CurrentUser;
use crate::config::{admin, apply_changes, blocking, edit_error, files, lease_ids, writable};
use axum::extract::{Extension, Json, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use toml::{Table, Value};
use tracing::{info, warn};
use watcher_config::Discovery;
use watcher_config::edit::{self, Edit, Kind, Object};
use watcher_store::{DiscoveredGuest, FoundGuest};

type Shared = State<Arc<Hub>>;
type User = Option<Extension<CurrentUser>>;

/// How often the loop looks for discoveries that are due.
const TICK: Duration = Duration::from_secs(15);
/// How long each API request may take.
const WAIT: Duration = Duration::from_secs(15);

/// The runs of each discovery, by name.
#[derive(Default)]
pub(crate) struct Runs {
    state: Mutex<HashMap<String, RunState>>,
    /// Wakes the loop for a run requested over the API.
    wake: tokio::sync::Notify,
}

#[derive(Debug, Clone, Default)]
struct RunState {
    /// When the last run ended.
    last: Option<SystemTime>,
    /// For the interval: `last` on the monotonic clock.
    last_at: Option<Instant>,
    error: Option<String>,
    running: bool,
    /// Run at once, whatever the interval.
    requested: bool,
}

impl Runs {
    fn get(&self, name: &str) -> RunState {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.get(name).cloned().unwrap_or_default()
    }

    fn update(&self, name: &str, f: impl FnOnce(&mut RunState)) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        f(state.entry(name.to_string()).or_default());
    }
}

/// Runs every configured discovery at its interval, until the process
/// ends. Needs the database, which keeps what was found.
pub async fn run(hub: Arc<Hub>) {
    let mut forgotten = Vec::new();
    loop {
        let config = hub.config();
        let names: Vec<String> = config.discoveries.iter().map(|d| d.name.clone()).collect();
        // Guests of discoveries taken out of the configuration.
        if names != forgotten
            && let Some(store) = &hub.store
        {
            match store.forget_discoveries_except(&names).await {
                Ok(_) => forgotten = names,
                Err(e) => warn!("discovery: cannot clean up: {e}"),
            }
        }
        for d in &config.discoveries {
            let state = hub.discovery.get(&d.name);
            let due = state.requested || state.last_at.is_none_or(|t| t.elapsed() >= d.interval);
            if due {
                run_one(&hub, d).await;
            }
        }
        tokio::select! {
            () = tokio::time::sleep(TICK) => {}
            () = hub.discovery.wake.notified() => {}
        }
    }
}

async fn run_one(hub: &Hub, d: &Discovery) {
    hub.discovery.update(&d.name, |s| {
        s.running = true;
        s.requested = false;
    });
    let result = discover(hub, d).await;
    let before = hub.discovery.get(&d.name).error;
    match &result {
        Err(e) if before.as_ref() != Some(e) => warn!("discovery {}: {e}", d.name),
        Ok(()) if before.is_some() => info!("discovery {}: working again", d.name),
        _ => {}
    }
    hub.discovery.update(&d.name, |s| {
        s.running = false;
        s.last = Some(SystemTime::now());
        s.last_at = Some(Instant::now());
        s.error = result.err();
    });
}

/// Asks the API, records the guests, and updates the hosts accepted
/// before.
async fn discover(hub: &Hub, d: &Discovery) -> Result<(), String> {
    let store = hub.store.as_ref().ok_or("discovery needs the database")?;
    let access = hub.ssh.get().ok_or("secrets are not set up")?;
    let guests = d.connection.guests(&access.pool, WAIT).await?;
    let found: Vec<FoundGuest> = guests
        .iter()
        .map(|g| FoundGuest {
            vmid: g.vmid,
            kind: g.kind.clone(),
            name: g.name.clone(),
            node: g.node.clone(),
            running: g.running,
            address: g.address.clone(),
        })
        .collect();
    store
        .sync_discovered(&d.name, &found)
        .await
        .map_err(|e| format!("cannot store what was found: {e}"))?;
    sync_hosts(hub, d).await
}

/// Brings the hosts of discovery `name` in line with the guests stored for
/// it, as after each run: without asking the API.
pub async fn update_hosts(hub: &Hub, name: &str) -> Result<(), String> {
    let d = hub
        .config()
        .discovery(name)
        .cloned()
        .ok_or_else(|| format!("no discovery {name:?}"))?;
    sync_hosts(hub, &d).await
}

/// Removes hosts whose guest is gone, and rewrites those whose address
/// changed.
async fn sync_hosts(hub: &Hub, d: &Discovery) -> Result<(), String> {
    let (Some(files), Some(store)) = (hub.config_files.get(), &hub.store) else {
        return Ok(());
    };
    let known = store
        .discovered(Some(&d.name))
        .await
        .map_err(|e| format!("cannot read what was found: {e}"))?;
    let accepted = accepted(hub, d).await?;
    let mut edits = Vec::new();
    for (vmid, host) in &accepted {
        let edit = match known.iter().find(|g| g.vmid == *vmid) {
            None => Edit {
                kind: Kind::Host,
                name: Some(host.name.clone()),
                fields: None,
                path: None,
            },
            Some(g) => {
                let fields = updated(&host.fields, d, g);
                if fields == host.fields {
                    continue;
                }
                Edit {
                    kind: Kind::Host,
                    name: Some(host.name.clone()),
                    fields: Some(fields),
                    path: None,
                }
            }
        };
        edits.push(edit);
    }
    if edits.is_empty() {
        return Ok(());
    }
    writable(files)
        .map_err(|_| "config_mode = \"files\": can't update the discovered hosts".to_string())?;
    let dir = files.dir.clone();
    let changes = blocking(move || edit::edit_many(&dir, &edits))
        .await
        .map_err(|e| format!("{e:?}"))?
        .map_err(|e| format!("cannot update {}: {e}", d.file()))?;
    let by = format!("discovery {}", d.name);
    let response = apply_changes(hub, files, by, changes, Vec::new())
        .await
        .map_err(|e| format!("cannot update {}: {e:?}", d.file()))?;
    match response.status() {
        // Someone has the file open: next run.
        StatusCode::LOCKED => Err(format!(
            "{} is being edited; the hosts are updated afterwards",
            d.file()
        )),
        s if s.is_success() => Ok(()),
        s => Err(format!("cannot update {}: {s}", d.file())),
    }
}

/// The hosts in the discovery's file, by VM ID.
async fn accepted(hub: &Hub, d: &Discovery) -> Result<BTreeMap<u32, Object>, String> {
    let Some(files) = hub.config_files.get() else {
        return Ok(BTreeMap::new());
    };
    let dir = files.dir.clone();
    let (objects, _) = blocking(move || edit::objects(&dir))
        .await
        .map_err(|e| format!("{e:?}"))?
        .map_err(|e| format!("cannot read the object files: {e}"))?;
    let file = d.file();
    Ok(objects
        .into_iter()
        .filter(|o| o.kind == Kind::Host && o.path == file)
        .filter_map(|o| Some((vmid_of(&o.fields)?, o)))
        .collect())
}

/// `proxmox.vmid` of a host as written.
fn vmid_of(fields: &Table) -> Option<u32> {
    let v = fields.get("proxmox")?.get("vmid")?;
    match v {
        Value::Integer(i) => u32::try_from(*i).ok(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// The check every accepted guest gets.
const GUEST_CHECK: &str = "proxmox-guest";

/// The host for guest `g`, as written to the discovery's file: on its
/// Proxmox VE host, with the check whether it runs.
fn host_fields(d: &Discovery, name: &str, g: &DiscoveredGuest) -> Table {
    let mut t = Table::new();
    t.insert("name".into(), Value::String(name.into()));
    if let Some(a) = &g.address {
        t.insert("address".into(), Value::String(a.clone()));
    }
    let mut proxmox = Table::new();
    proxmox.insert("host".into(), Value::String(d.name.clone()));
    proxmox.insert("vmid".into(), Value::Integer(i64::from(g.vmid)));
    t.insert("proxmox".into(), Value::Table(proxmox));
    t.insert(
        "services".into(),
        Value::Array(vec![Value::String(GUEST_CHECK.into())]),
    );
    // Without an address there is nothing to ping: the API says whether
    // it is UP.
    if g.address.is_none() {
        t.insert("host_check".into(), Value::String(GUEST_CHECK.into()));
    }
    t
}

/// An accepted host with what discovery knows brought up to date: the
/// address (kept while the guest is stopped) and `proxmox`. Everything
/// else, e.g. services added in the file, stays as written.
fn updated(fields: &Table, d: &Discovery, g: &DiscoveredGuest) -> Table {
    let mut t = fields.clone();
    if let Some(a) = &g.address {
        let had = t.insert("address".into(), Value::String(a.clone()));
        // It had no address: now ping can say whether it is UP.
        if had.is_none() && t.get("host_check").and_then(Value::as_str) == Some(GUEST_CHECK) {
            t.remove("host_check");
        }
    }
    let mut proxmox = Table::new();
    proxmox.insert("host".into(), Value::String(d.name.clone()));
    proxmox.insert("vmid".into(), Value::Integer(i64::from(g.vmid)));
    t.insert("proxmox".into(), Value::Table(proxmox));
    t
}

/// A host name for a guest: its name, with anything but letters, digits,
/// `.`, `-` and `_` replaced by `-`.
fn host_name(guest: &str) -> String {
    let name: String = guest
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect();
    name.trim_matches('-').to_string()
}

#[derive(Debug, Serialize)]
pub(crate) struct DiscoveryView {
    /// The Proxmox VE host.
    name: String,
    /// The API it asks.
    url: String,
    /// Seconds.
    interval: u64,
    /// Where accepted hosts go, relative to the object directory.
    file: String,
    last_run: Option<String>,
    error: Option<String>,
    running: bool,
    guests: Vec<GuestView>,
}

#[derive(Debug, Serialize)]
pub(crate) struct GuestView {
    #[serde(flatten)]
    guest: DiscoveredGuest,
    /// The host it was accepted as.
    host: Option<String>,
}

/// `GET /discovery`: every discovery with the guests it found.
pub(crate) async fn list(
    State(hub): Shared,
    user: User,
) -> Result<Json<Vec<DiscoveryView>>, ApiError> {
    admin(&hub, &user)?;
    let store = hub.store.as_ref().ok_or(ApiError::NoDatabase)?;
    let config = hub.config();
    let all = store.discovered(None).await?;
    let mut out = Vec::new();
    for d in &config.discoveries {
        let accepted = accepted(&hub, d).await.map_err(ApiError::Internal)?;
        let state = hub.discovery.get(&d.name);
        let guests = all
            .iter()
            .filter(|g| g.discovery == d.name)
            .map(|g| GuestView {
                host: accepted.get(&g.vmid).map(|o| o.name.clone()),
                guest: g.clone(),
            })
            .collect();
        out.push(DiscoveryView {
            name: d.name.clone(),
            url: d.connection.url.clone(),
            interval: d.interval.as_secs(),
            file: d.file(),
            last_run: state
                .last
                .map(|t| humantime::format_rfc3339_millis(t).to_string()),
            error: state.error,
            running: state.running || state.requested,
            guests,
        });
    }
    Ok(Json(out))
}

fn find(hub: &Hub, name: &str) -> Result<Discovery, ApiError> {
    hub.config()
        .discovery(name)
        .cloned()
        .ok_or_else(|| ApiError::NotFound(format!("no discovery {name:?}")))
}

/// `POST /discovery/{name}/run`: runs it now.
pub(crate) async fn run_now(
    State(hub): Shared,
    user: User,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    admin(&hub, &user)?;
    find(&hub, &name)?;
    hub.discovery.update(&name, |s| s.requested = true);
    hub.discovery.wake.notify_one();
    Ok(StatusCode::ACCEPTED)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Guests {
    vmids: Vec<u32>,
}

/// `POST /discovery/{name}/accept`: writes the guests as hosts to the
/// discovery's file, and reloads.
pub(crate) async fn accept(
    State(hub): Shared,
    user: User,
    Path(name): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Guests>,
) -> Result<Response, ApiError> {
    let by = admin(&hub, &user)?;
    let d = find(&hub, &name)?;
    let files = files(&hub)?;
    writable(files)?;
    let store = hub.store.as_ref().ok_or(ApiError::NoDatabase)?;
    let known = store.discovered(Some(&name)).await?;
    let accepted = accepted(&hub, &d).await.map_err(ApiError::Internal)?;
    let config = hub.config();
    let mut taken: Vec<String> = config.hosts.iter().map(|h| h.name.clone()).collect();
    let mut edits = Vec::new();
    for vmid in &body.vmids {
        let Some(g) = known.iter().find(|g| g.vmid == *vmid) else {
            return Err(ApiError::BadRequest(format!(
                "{name} found no guest {vmid}"
            )));
        };
        if let Some(host) = accepted.get(vmid) {
            return Err(ApiError::Conflict(format!(
                "guest {vmid} is the host {} already",
                host.name
            )));
        }
        // Its own name if free, else with its VM ID.
        let mut host = host_name(&g.name);
        if host.is_empty() || taken.contains(&host) {
            host = format!("{}-{vmid}", if host.is_empty() { &g.kind } else { &host });
        }
        if taken.contains(&host) {
            return Err(ApiError::Conflict(format!(
                "a host {host:?} exists already"
            )));
        }
        taken.push(host.clone());
        edits.push(Edit {
            kind: Kind::Host,
            name: None,
            fields: Some(host_fields(&d, &host, g)),
            path: Some(d.file()),
        });
    }
    if edits.is_empty() {
        return Err(ApiError::BadRequest("no guests given".into()));
    }
    let dir = files.dir.clone();
    match blocking(move || edit::edit_many(&dir, &edits)).await? {
        Ok(changes) => {
            store.ignore_discovered(&name, &body.vmids, false).await?;
            apply_changes(&hub, files, by, changes, lease_ids(&headers)).await
        }
        Err(err) => edit_error(files, err),
    }
}

/// `POST /discovery/{name}/remove`: removes the hosts of accepted guests;
/// they are offered again.
pub(crate) async fn remove(
    State(hub): Shared,
    user: User,
    Path(name): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Guests>,
) -> Result<Response, ApiError> {
    let by = admin(&hub, &user)?;
    let d = find(&hub, &name)?;
    let files = files(&hub)?;
    writable(files)?;
    let accepted = accepted(&hub, &d).await.map_err(ApiError::Internal)?;
    let mut edits = Vec::new();
    for vmid in &body.vmids {
        let Some(host) = accepted.get(vmid) else {
            return Err(ApiError::BadRequest(format!("guest {vmid} is not a host")));
        };
        edits.push(Edit {
            kind: Kind::Host,
            name: Some(host.name.clone()),
            fields: None,
            path: None,
        });
    }
    if edits.is_empty() {
        return Err(ApiError::BadRequest("no guests given".into()));
    }
    let dir = files.dir.clone();
    match blocking(move || edit::edit_many(&dir, &edits)).await? {
        Ok(changes) => apply_changes(&hub, files, by, changes, lease_ids(&headers)).await,
        Err(err) => edit_error(files, err),
    }
}

/// `POST /discovery/{name}/ignore`: no longer offers the guests.
pub(crate) async fn ignore(
    State(hub): Shared,
    user: User,
    Path(name): Path<String>,
    Json(body): Json<Guests>,
) -> Result<Response, ApiError> {
    set_ignored(&hub, &user, &name, &body.vmids, true).await
}

/// `POST /discovery/{name}/unignore`: offers the guests again.
pub(crate) async fn unignore(
    State(hub): Shared,
    user: User,
    Path(name): Path<String>,
    Json(body): Json<Guests>,
) -> Result<Response, ApiError> {
    set_ignored(&hub, &user, &name, &body.vmids, false).await
}

async fn set_ignored(
    hub: &Hub,
    user: &User,
    name: &str,
    vmids: &[u32],
    ignored: bool,
) -> Result<Response, ApiError> {
    let by = admin(hub, user)?;
    find(hub, name)?;
    let store = hub.store.as_ref().ok_or(ApiError::NoDatabase)?;
    let changed = store.ignore_discovered(name, vmids, ignored).await?;
    if changed == 0 {
        return Err(ApiError::BadRequest(format!(
            "{name} found none of these guests"
        )));
    }
    let what = if ignored { "ignores" } else { "offers again" };
    info!("{by}: discovery {name} {what} {vmids:?}");
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(host_name("jellyfin"), "jellyfin");
        assert_eq!(host_name("web 01 (old)"), "web-01--old");
        assert_eq!(host_name("ü"), "");
    }
}
