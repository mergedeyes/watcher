//! Nodes: which are connected over the node link (see `link`), and the API
//! to add and remove them. Listing is for everyone; changes are for admins.

use crate::Hub;
use crate::api::ApiError;
use crate::auth::{CurrentUser, admin_user};
use crate::ca::Ca;
use crate::view::time;
use axum::extract::{Extension, Json, Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::sync::oneshot;
use watcher_proto::JoinToken;
use watcher_store::Store;

type Shared = State<Arc<Hub>>;
type User = Option<Extension<CurrentUser>>;

/// How long a join token can be used.
pub const TOKEN_TTL: Duration = Duration::from_secs(24 * 3600);

/// What nodes need from the hub: the CA that signs their certificates, and
/// the address they reach the hub at.
pub struct NodeLink {
    pub ca: Arc<Ca>,
    /// `https://host:port`, for the join command.
    pub url: String,
}

/// An open session of a node.
struct Session {
    id: u64,
    cert_fp: String,
    since: SystemTime,
    /// Ends the session, with the reason for the node.
    close: oneshot::Sender<&'static str>,
}

/// The nodes connected right now.
#[derive(Default)]
pub(crate) struct Registry {
    sessions: Mutex<HashMap<String, Session>>,
    next: AtomicU64,
}

impl Registry {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Session>> {
        self.sessions.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Registers a new session of node `name`, closing an older one. The
    /// receiver gets a reason when the hub ends the session.
    pub(crate) fn open(&self, name: &str, cert_fp: &str) -> (u64, oneshot::Receiver<&'static str>) {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (close, closed) = oneshot::channel();
        let session = Session {
            id,
            cert_fp: cert_fp.to_string(),
            since: SystemTime::now(),
            close,
        };
        if let Some(old) = self.lock().insert(name.to_string(), session) {
            let _ = old
                .close
                .send("replaced by a newer connection of the same node");
        }
        (id, closed)
    }

    /// Forgets session `id` of node `name`, which ended.
    pub(crate) fn ended(&self, name: &str, id: u64) {
        let mut sessions = self.lock();
        if sessions.get(name).is_some_and(|s| s.id == id) {
            sessions.remove(name);
        }
    }

    /// Ends the session of node `name`, if it has one.
    pub(crate) fn close(&self, name: &str, why: &'static str) {
        if let Some(session) = self.lock().remove(name) {
            let _ = session.close.send(why);
        }
    }

    /// Ends the sessions whose certificate is no longer their node's, after
    /// a change outside this process (`watcher node remove`). `valid` maps
    /// node names to their certificate's fingerprint. Returns the names.
    pub(crate) fn close_revoked(&self, valid: &HashMap<String, String>) -> Vec<String> {
        let mut sessions = self.lock();
        let revoked: Vec<String> = sessions
            .iter()
            .filter(|(name, s)| valid.get(*name) != Some(&s.cert_fp))
            .map(|(name, _)| name.clone())
            .collect();
        for name in &revoked {
            if let Some(session) = sessions.remove(name) {
                let _ = session.close.send("the node was removed on the hub");
            }
        }
        revoked
    }

    /// Connected nodes and since when.
    pub(crate) fn connected(&self) -> HashMap<String, SystemTime> {
        self.lock()
            .iter()
            .map(|(name, s)| (name.clone(), s.since))
            .collect()
    }
}

/// Node names: 1 to 64 letters, digits, `.`, `_` and `-`, starting with a
/// letter or digit, as they also appear in host names.
pub fn check_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    match ok {
        true => Ok(()),
        false => Err(format!(
            "node name {name:?}: 1 to 64 letters, digits, '.', '_' and '-', starting with a letter or digit"
        )),
    }
}

/// A new join token for node `name` (see [`check_name`]), valid for
/// [`TOKEN_TTL`]; the node is created if needed. A node that has joined
/// already gets one only with `rejoin`.
pub async fn new_join_token(
    store: &Store,
    ca: &Ca,
    name: &str,
    rejoin: bool,
) -> watcher_store::Result<(JoinToken, SystemTime)> {
    let token = JoinToken::generate(ca.fingerprint());
    let expires = SystemTime::now() + TOKEN_TTL;
    store
        .set_node_token(name, &JoinToken::hash(&token.secret), expires, rejoin)
        .await?;
    Ok((token, expires))
}

/// The command that joins a node.
pub fn join_command(url: &str, token: &JoinToken) -> String {
    format!("watcher node join {url} --token {token}")
}

fn link(hub: &Hub) -> Result<(&NodeLink, &Store), ApiError> {
    let link = hub.link.get().ok_or_else(|| {
        ApiError::Unavailable("nodes connect to a hub: set role = \"hub\" in watcher.toml".into())
    })?;
    let store = hub.store.as_deref().ok_or(ApiError::NoDatabase)?;
    Ok((link, store))
}

#[derive(Debug, Serialize)]
pub(crate) struct NodeView {
    name: String,
    /// `connected`; `offline` (joined, not connected); `new` (added or
    /// named by hosts, not joined yet).
    state: &'static str,
    connected_since: Option<String>,
    joined: Option<String>,
    /// When the unused join token expires.
    token_expires: Option<String>,
    last_seen: Option<String>,
    version: Option<String>,
    hostname: Option<String>,
    address: Option<String>,
    /// Hosts in the configuration with this node.
    hosts: Vec<String>,
}

/// `GET /nodes`: nodes that have joined or have a join token, and node
/// names that hosts use.
pub(crate) async fn list(State(hub): Shared) -> Result<Json<Vec<NodeView>>, ApiError> {
    let (_, store) = link(&hub)?;
    let config = hub.config();
    let mut hosts: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for host in &config.hosts {
        if let Some(node) = &host.node {
            hosts.entry(node).or_default().push(host.name.clone());
        }
    }
    let connected = hub.nodes.connected();
    let now = SystemTime::now();
    let mut list = Vec::new();
    for node in store.nodes().await? {
        let named = hosts.remove(node.name.as_str()).unwrap_or_default();
        let token = node.token_expires.filter(|t| *t > now);
        if node.joined.is_none() && token.is_none() && named.is_empty() {
            // Only named by hosts that left the configuration.
            continue;
        }
        let since = connected.get(&node.name).copied();
        let state = match (since, node.joined) {
            (Some(_), _) => "connected",
            (None, Some(_)) => "offline",
            (None, None) => "new",
        };
        list.push(NodeView {
            state,
            connected_since: since.map(time),
            joined: node.joined.map(time),
            token_expires: token.map(time),
            last_seen: since.map_or(node.last_seen, |_| Some(now)).map(time),
            version: node.version,
            hostname: node.hostname,
            address: node.address,
            hosts: named,
            name: node.name,
        });
    }
    // Names of hosts added since the last sync with the database.
    for (name, named) in hosts {
        list.push(NodeView {
            name: name.to_string(),
            state: "new",
            connected_since: None,
            joined: None,
            token_expires: None,
            last_seen: None,
            version: None,
            hostname: None,
            address: None,
            hosts: named,
        });
    }
    list.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Json(list))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NewNode {
    name: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct Token {
    name: String,
    token: String,
    expires: String,
    /// Run on the node's machine.
    command: String,
}

async fn token(hub: &Hub, user: &User, name: &str, rejoin: bool) -> Result<Token, ApiError> {
    let by = admin_user(hub, user)?;
    let (link, store) = link(hub)?;
    check_name(name).map_err(ApiError::BadRequest)?;
    let (token, expires) = new_join_token(store, &link.ca, name, rejoin).await?;
    tracing::info!("{by}: made a join token for node {name}");
    Ok(Token {
        name: name.to_string(),
        command: join_command(&link.url, &token),
        token: token.to_string(),
        expires: time(expires),
    })
}

/// `POST /nodes`: adds a node, or gives one that hosts name a join token.
pub(crate) async fn add(
    State(hub): Shared,
    user: User,
    Json(body): Json<NewNode>,
) -> Result<(StatusCode, Json<Token>), ApiError> {
    let token = token(&hub, &user, body.name.trim(), false).await?;
    Ok((StatusCode::CREATED, Json(token)))
}

/// `POST /nodes/{name}/token`: a new join token, also for a node that has
/// joined already (a reinstalled machine). Its certificate works until it
/// joins again.
pub(crate) async fn new_token(
    State(hub): Shared,
    user: User,
    Path(name): Path<String>,
) -> Result<Json<Token>, ApiError> {
    Ok(Json(token(&hub, &user, &name, true).await?))
}

/// `DELETE /nodes/{name}`: its certificate and join token stop working,
/// and its connection ends.
pub(crate) async fn remove(
    State(hub): Shared,
    user: User,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let by = admin_user(&hub, &user)?;
    let (_, store) = link(&hub)?;
    if !store.remove_node(&name).await? {
        return Err(ApiError::NotFound(format!("no node {name}")));
    }
    hub.nodes.close(&name, "the node was removed on the hub");
    tracing::info!("{by}: removed node {name}");
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        for ok in ["site-a", "dc1.example", "a", "A_1", &"x".repeat(64)] {
            assert!(check_name(ok).is_ok(), "{ok}");
        }
        for bad in ["", "-a", ".a", "a b", "a/b", "ä", &"x".repeat(65)] {
            assert!(check_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_new_session_replaces_the_old_one() {
        let registry = Registry::default();
        let (first, mut closed) = registry.open("a", "fp1");
        let (second, _) = registry.open("a", "fp1");
        assert!(closed.try_recv().is_ok());
        // The first session ending doesn't drop the second.
        registry.ended("a", first);
        assert!(registry.connected().contains_key("a"));
        registry.ended("a", second);
        assert!(registry.connected().is_empty());
    }

    #[test]
    fn revoked_sessions_close() {
        let registry = Registry::default();
        let (_, mut a) = registry.open("a", "fp-a");
        let (_, mut b) = registry.open("b", "fp-b");
        let valid = HashMap::from([("a".to_string(), "fp-a".to_string())]);
        assert_eq!(registry.close_revoked(&valid), ["b"]);
        assert!(a.try_recv().is_err());
        assert_eq!(b.try_recv(), Ok("the node was removed on the hub"));
    }
}
