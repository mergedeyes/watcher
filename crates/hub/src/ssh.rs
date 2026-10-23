//! Checks over SSH: the vault on the database (secrets sealed with
//! `secret.key`, host keys pinned on first use), and the API for both.
//! Admins only.

use crate::Hub;
use crate::api::ApiError;
use crate::auth::{CurrentUser, admin_user};
use axum::extract::{Extension, Json, Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use watcher_config::{Config, RunOn, Service};
use watcher_exec::ssh::{
    self, BoxFuture, Credential, Kind, Pool, PublicKey, Remote, Sealer, Target, Vault,
};
use watcher_store::{HostKey, SecretInfo, Store};

type Shared = State<Arc<Hub>>;
type User = Option<Extension<CurrentUser>>;

/// Secrets and host keys in the database.
pub struct StoreVault {
    store: Arc<Store>,
    data_dir: PathBuf,
    /// Loaded on first use: `secret.key` may be created later.
    sealer: Mutex<Option<Arc<Sealer>>>,
}

impl StoreVault {
    pub fn new(store: Arc<Store>, data_dir: PathBuf) -> Self {
        StoreVault {
            store,
            data_dir,
            sealer: Mutex::new(None),
        }
    }

    /// The sealer; with `create`, `secret.key` is made if missing.
    fn sealer(&self, create: bool) -> Result<Arc<Sealer>, String> {
        let mut cached = self.sealer.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(sealer) = &*cached {
            return Ok(sealer.clone());
        }
        let sealer = Arc::new(match create {
            true => Sealer::open_or_create(&self.data_dir)?,
            false => Sealer::open(&self.data_dir)?,
        });
        *cached = Some(sealer.clone());
        Ok(sealer)
    }
}

impl Vault for StoreVault {
    fn credential<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Credential, String>> {
        Box::pin(async move {
            let (kind, sealed) = self
                .store
                .secret(name)
                .await
                .map_err(|e| format!("database: {e}"))?
                .ok_or("no such secret (watcher secret, or Configuration → SSH)")?;
            let kind = Kind::parse(&kind).ok_or_else(|| format!("unknown kind {kind:?}"))?;
            self.sealer(false)?.unseal(name, kind, &sealed)
        })
    }

    fn check_host_key<'a>(
        &'a self,
        address: &'a str,
        port: u16,
        key: &'a PublicKey,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let line = ssh::key_line(key);
            let trusted = self
                .store
                .check_host_key(address, port, &line)
                .await
                .map_err(|e| format!("database: {e}"))?;
            match trusted {
                true => Ok(()),
                false => Err(format!(
                    "host key changed ({}); accept: Configuration → SSH, or watcher hostkey accept {}",
                    ssh::fingerprint(&line).unwrap_or_default(),
                    host_port(address, port),
                )),
            }
        })
    }
}

/// `db1`, or `db1:2222` for other ports.
fn host_port(address: &str, port: u16) -> String {
    match port {
        22 => address.to_string(),
        _ => format!("{address}:{port}"),
    }
}

/// SSH for the hub: the connections (for test runs) and the vault (for
/// the API); see [`Hub::enable_ssh`].
pub(crate) struct Access {
    pub pool: Arc<Pool>,
    /// `None` without database or data directory: `pool` then says why.
    pub vault: Option<Arc<StoreVault>>,
}

/// How `service` runs on its host, if it does (`run_on = "host"`).
pub fn remote(config: &Config, service: &Service) -> Option<Remote> {
    if service.settings.run_on != RunOn::Host {
        return None;
    }
    let host = config.host(service);
    let ssh = host.ssh.as_ref()?;
    Some(Remote {
        target: Target {
            address: host.address.clone(),
            port: ssh.port,
            user: ssh.user.clone(),
            secret: ssh.secret.clone(),
        },
        copy: service.settings.copy,
    })
}

fn access(hub: &Hub) -> Result<(&Access, &StoreVault), ApiError> {
    let access = hub
        .ssh
        .get()
        .ok_or_else(|| ApiError::Unavailable("SSH is not set up".into()))?;
    let vault = access.vault.as_deref().ok_or_else(|| {
        ApiError::Unavailable("SSH needs a database and watcher.toml (data_dir)".into())
    })?;
    Ok((access, vault))
}

#[derive(Debug, Serialize)]
pub(crate) struct SecretView {
    #[serde(flatten)]
    info: SecretInfo,
    fingerprint: Option<String>,
    /// Hosts that log in with it.
    hosts: Vec<String>,
}

fn secret_view(config: &Config, info: SecretInfo) -> SecretView {
    SecretView {
        fingerprint: info.public_key.as_deref().and_then(ssh::fingerprint),
        hosts: config
            .hosts
            .iter()
            .filter(|h| h.ssh.as_ref().is_some_and(|s| s.secret == info.name))
            .map(|h| h.name.clone())
            .collect(),
        info,
    }
}

/// `GET /ssh/secrets`
pub(crate) async fn secrets(
    State(hub): Shared,
    user: User,
) -> Result<Json<Vec<SecretView>>, ApiError> {
    admin_user(&hub, &user)?;
    let (_, vault) = access(&hub)?;
    let config = hub.config();
    let list = vault.store.secrets().await?;
    Ok(Json(
        list.into_iter()
            .map(|info| secret_view(&config, info))
            .collect(),
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum NewKind {
    /// A new ed25519 key.
    Generate,
    /// An existing private key.
    Key,
    Password,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NewSecret {
    name: String,
    kind: NewKind,
    private_key: Option<String>,
    passphrase: Option<String>,
    password: Option<String>,
    /// Replace a secret of the same name.
    #[serde(default)]
    replace: bool,
}

/// `POST /ssh/secrets`: generates or stores a key, or stores a password.
pub(crate) async fn create_secret(
    State(hub): Shared,
    user: User,
    Json(body): Json<NewSecret>,
) -> Result<(StatusCode, Json<SecretView>), ApiError> {
    let by = admin_user(&hub, &user)?;
    let (access, vault) = access(&hub)?;
    let name = body.name.trim();
    check_name(name).map_err(|m| ApiError::BadRequest(m.into()))?;
    let bad = |m: &str| ApiError::BadRequest(m.into());
    let credential = match body.kind {
        NewKind::Generate => Credential::generate(&format!("watcher-{name}")),
        NewKind::Key => {
            let text = body
                .private_key
                .as_deref()
                .ok_or_else(|| bad("private_key missing"))?;
            let passphrase = body.passphrase.as_deref().filter(|p| !p.is_empty());
            Credential::import(text, passphrase)
        }
        NewKind::Password => match body.password {
            Some(p) if !p.is_empty() => Ok(Credential::Password(p)),
            _ => return Err(bad("password missing")),
        },
    }
    .map_err(ApiError::BadRequest)?;
    let sealed = vault
        .sealer(true)
        .and_then(|s| s.seal(name, &credential))
        .map_err(ApiError::Internal)?;
    let public_key = credential.public_key();
    let kind = credential.kind().as_str();
    vault
        .store
        .put_secret(name, kind, public_key.as_deref(), &sealed, body.replace)
        .await?;
    access.pool.reset_failed();
    tracing::info!("{by}: stored secret {name} ({kind})");
    let now = SystemTime::now();
    let info = SecretInfo {
        name: name.to_string(),
        kind: kind.to_string(),
        public_key,
        created: now,
        updated: now,
    };
    Ok((StatusCode::CREATED, Json(secret_view(&hub.config(), info))))
}

/// Secret names are referenced from conf.d (`ssh.secret`).
pub fn check_name(name: &str) -> Result<(), &'static str> {
    match name.is_empty()
        || name.len() > 64
        || name
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '/')
    {
        true => Err("names are 1 to 64 characters without spaces or /"),
        false => Ok(()),
    }
}

/// `DELETE /ssh/secrets/{name}`
pub(crate) async fn delete_secret(
    State(hub): Shared,
    user: User,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let by = admin_user(&hub, &user)?;
    let (_, vault) = access(&hub)?;
    if !vault.store.delete_secret(&name).await? {
        return Err(ApiError::NotFound(format!("no secret {name:?}")));
    }
    tracing::info!("{by}: deleted secret {name}");
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize)]
pub(crate) struct HostKeyView {
    #[serde(flatten)]
    key: HostKey,
    fingerprint: Option<String>,
    offered_fingerprint: Option<String>,
    /// Hosts at this address and port.
    hosts: Vec<String>,
}

fn host_key_view(config: &Config, key: HostKey) -> HostKeyView {
    HostKeyView {
        fingerprint: ssh::fingerprint(&key.key),
        offered_fingerprint: key.offered.as_deref().and_then(ssh::fingerprint),
        hosts: config
            .hosts
            .iter()
            .filter(|h| {
                h.address == key.address && h.ssh.as_ref().is_some_and(|s| s.port == key.port)
            })
            .map(|h| h.name.clone())
            .collect(),
        key,
    }
}

/// `GET /ssh/hostkeys`
pub(crate) async fn host_keys(
    State(hub): Shared,
    user: User,
) -> Result<Json<Vec<HostKeyView>>, ApiError> {
    admin_user(&hub, &user)?;
    let (_, vault) = access(&hub)?;
    let config = hub.config();
    let list = vault.store.host_keys().await?;
    Ok(Json(
        list.into_iter()
            .map(|k| host_key_view(&config, k))
            .collect(),
    ))
}

/// `POST /ssh/hostkeys/{address}/{port}/accept`: pins the offered key.
pub(crate) async fn accept_host_key(
    State(hub): Shared,
    user: User,
    Path((address, port)): Path<(String, u16)>,
) -> Result<Json<HostKeyView>, ApiError> {
    let by = admin_user(&hub, &user)?;
    let (access, vault) = access(&hub)?;
    let key = vault
        .store
        .accept_host_key(&address, port)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!("no new key for {}", host_port(&address, port)))
        })?;
    access.pool.reset_failed();
    tracing::info!(
        "{by}: accepted the new host key of {}",
        host_port(&address, port)
    );
    Ok(Json(host_key_view(&hub.config(), key)))
}

/// `DELETE /ssh/hostkeys/{address}/{port}`: the next key is pinned.
pub(crate) async fn forget_host_key(
    State(hub): Shared,
    user: User,
    Path((address, port)): Path<(String, u16)>,
) -> Result<StatusCode, ApiError> {
    let by = admin_user(&hub, &user)?;
    let (access, vault) = access(&hub)?;
    if !vault.store.forget_host_key(&address, port).await? {
        return Err(ApiError::NotFound(format!(
            "no host key for {}",
            host_port(&address, port)
        )));
    }
    access.pool.reset_failed();
    tracing::info!("{by}: forgot the host key of {}", host_port(&address, port));
    Ok(StatusCode::NO_CONTENT)
}
