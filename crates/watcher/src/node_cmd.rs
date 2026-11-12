//! Nodes: `watcher node` on the hub (add, token, list, remove) and on the
//! node's machine (join); the node link when running as hub or node.

use crate::user_cmd::DbArgs;
use anyhow::{Context, Result, anyhow, bail};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::signal::unix::{SignalKind, signal};
use tokio::task::JoinHandle;
use tracing::info;
use watcher_config::{Bootstrap, InstanceRole};
use watcher_hub::ca::Ca;
use watcher_hub::{Hub, NodeLink, check_name, join_command, new_join_token, serve_node_link};
use watcher_proto::{JoinToken, hub_address, hub_url};

#[derive(clap::Subcommand)]
pub enum NodeCommand {
    /// Add a node on the hub and print the command that joins it.
    ///
    /// The join token in the command works once, within 24 hours.
    Add { name: String },
    /// Print a new join command for a node, e.g. for a reinstalled machine.
    ///
    /// The node's current certificate keeps working until it joins again.
    Token { name: String },
    /// List the hub's nodes.
    List,
    /// Remove a node from the hub.
    ///
    /// Its certificate and join token stop working; a running hub ends its
    /// connection within a minute.
    Remove { name: String },
    /// Join a hub: run this on the node's machine.
    ///
    /// Keeps the node's key and certificates in data_dir/node/ and writes
    /// role = "node" and the hub's address to watcher.toml (-c, default
    /// ./watcher.toml or /etc/watcher/watcher.toml; created if missing).
    Join {
        /// The hub's address for nodes, e.g. https://hub.example.com:7443.
        hub: String,
        /// The join token from the hub (watcher node add, or the Nodes page).
        #[arg(long)]
        token: String,
    },
}

pub async fn run(command: NodeCommand, db: DbArgs) -> Result<ExitCode> {
    if let NodeCommand::Join { hub, token } = command {
        return join(&hub, &token, db.config_path()).await;
    }
    let bootstrap = db.bootstrap()?;
    if bootstrap.role != InstanceRole::Hub {
        bail!(
            "nodes connect to a hub; this watcher.toml has role = {:?}",
            bootstrap.role.as_str()
        );
    }
    let store = db.store().await?;
    let (name, rejoin) = match command {
        NodeCommand::Add { name } => (name, false),
        NodeCommand::Token { name } => (name, true),
        NodeCommand::List => {
            list(&store).await?;
            return Ok(ExitCode::SUCCESS);
        }
        NodeCommand::Remove { name } => {
            if !store.remove_node(&name).await? {
                bail!("no node {name:?}");
            }
            println!("removed node {name}; a running hub ends its connection within a minute");
            return Ok(ExitCode::SUCCESS);
        }
        NodeCommand::Join { .. } => unreachable!("handled above"),
    };
    let name = name.trim();
    check_name(name).map_err(|e| anyhow!(e))?;
    let plan = link_plan(&bootstrap)?;
    let ca = Ca::open_or_create(&bootstrap.data_dir).map_err(|e| anyhow!(e))?;
    let (token, expires) = new_join_token(&store, &ca, name, rejoin).await?;
    let until = humantime::format_rfc3339_seconds(expires);
    eprintln!("Run this on the node's machine (the token works once, until {until}):");
    eprintln!();
    println!("{}", join_command(&plan.url, &token));
    Ok(ExitCode::SUCCESS)
}

async fn list(store: &watcher_store::Store) -> Result<()> {
    let nodes = store.nodes().await?;
    let now = SystemTime::now();
    let mut any = false;
    for n in nodes {
        let time = |t: SystemTime| humantime::format_rfc3339_seconds(t).to_string();
        let what = match (n.joined, n.token_expires.filter(|t| *t > now)) {
            (Some(joined), _) => {
                let seen = n.last_seen.map_or("never connected".into(), |t| {
                    format!("last seen {}", time(t))
                });
                let on = match (&n.hostname, &n.address) {
                    (Some(host), Some(address)) => format!(", on {host} ({address})"),
                    (Some(host), None) => format!(", on {host}"),
                    _ => String::new(),
                };
                let version = n
                    .version
                    .map_or(String::new(), |v| format!(", version {v}"));
                format!("joined {}, {seen}{version}{on}", time(joined))
            }
            (None, Some(expires)) => {
                format!("not joined; join token valid until {}", time(expires))
            }
            // Only named by hosts.
            (None, None) => continue,
        };
        any = true;
        println!("{:<24} {what}", n.name);
    }
    if !any {
        println!("no nodes yet; add one with: watcher node add <name>");
    }
    Ok(())
}

/// Where a hub listens for nodes and how they reach it.
pub struct LinkPlan {
    listen: SocketAddr,
    /// For join commands.
    url: String,
    data_dir: PathBuf,
}

/// The node link of a hub (`role = "hub"`), from its `[hub]` settings.
pub fn link_plan(bootstrap: &Bootstrap) -> Result<LinkPlan> {
    let settings = bootstrap.hub.clone().unwrap_or_default();
    let listen: SocketAddr = settings
        .listen
        .parse()
        .with_context(|| format!("[hub] listen: invalid address {:?}", settings.listen))?;
    let url = match &settings.url {
        Some(url) => {
            let (host, port) = hub_address(url).map_err(|e| anyhow!("[hub] url: {e}"))?;
            hub_url(&host, port)
        }
        None => {
            let host = bootstrap
                .public_url
                .as_deref()
                .and_then(url_host)
                .map(str::to_string)
                .or_else(|| (!listen.ip().is_unspecified()).then(|| listen.ip().to_string()))
                .unwrap_or_else(crate::init_cmd::hostname);
            hub_url(&host, listen.port())
        }
    };
    Ok(LinkPlan {
        listen,
        url,
        data_dir: bootstrap.data_dir.clone(),
    })
}

/// The host of an http(s) URL.
fn url_host(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let host = match rest.strip_prefix('[') {
        Some(v6) => v6.split(']').next()?,
        None => rest.split(['/', ':', '?', '#']).next()?,
    };
    (!host.is_empty()).then_some(host)
}

/// Lets nodes join and connect to `hub`, which needs a database.
pub fn start_link(hub: &Arc<Hub>, plan: LinkPlan, database: bool) -> Result<JoinHandle<()>> {
    if !database {
        bail!("role = \"hub\" needs a database: the hub keeps its nodes there");
    }
    let ca = Ca::open_or_create(&plan.data_dir).map_err(|e| anyhow!(e))?;
    hub.enable_nodes(NodeLink {
        ca: Arc::new(ca),
        url: plan.url.clone(),
    });
    let listener = std::net::TcpListener::bind(plan.listen)
        .with_context(|| format!("cannot listen for nodes on {}", plan.listen))?;
    let server = serve_node_link(hub.clone(), listener).map_err(|e| anyhow!(e))?;
    info!("nodes connect on {} as {}", plan.listen, plan.url);
    Ok(server)
}

/// `watcher run` with `role = "node"`: keeps the link to the hub until
/// stopped.
pub async fn run_node(bootstrap: &Bootstrap) -> Result<ExitCode> {
    let settings = bootstrap
        .node
        .as_ref()
        .expect("checked when watcher.toml was loaded");
    let mut terminate = signal(SignalKind::terminate())?;
    let shutdown = async move {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    };
    info!("running as a node of the hub at {}", settings.hub);
    watcher_node::run(
        &settings.hub,
        &bootstrap.data_dir,
        &crate::init_cmd::hostname(),
        shutdown,
    )
    .await?;
    info!("stopping");
    Ok(ExitCode::SUCCESS)
}

/// `watcher node join`: joins the hub, then makes watcher.toml say so.
async fn join(hub: &str, token: &str, config: Option<&Path>) -> Result<ExitCode> {
    let token: JoinToken = token.parse().map_err(|e: String| anyhow!(e))?;
    let (host, port) = hub_address(hub).map_err(|e| anyhow!(e))?;
    let url = hub_url(&host, port);
    let path = match config {
        Some(path) => path.to_path_buf(),
        None => ["watcher.toml", "/etc/watcher/watcher.toml"]
            .into_iter()
            .map(PathBuf::from)
            .find(|p| p.exists())
            .unwrap_or_else(|| PathBuf::from("watcher.toml")),
    };
    let base = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    // Read as a document, not as a Bootstrap: `[node]` is only added below.
    let mut doc = match std::fs::read_to_string(&path) {
        Ok(text) => {
            let doc: toml_edit::DocumentMut = text
                .parse()
                .with_context(|| format!("cannot read {}", path.display()))?;
            let role = doc
                .get("role")
                .and_then(|r| r.as_str())
                .unwrap_or("standalone");
            if role != "node" {
                bail!(
                    "{} has role = {role:?}; a node runs from its own watcher.toml: \
                     pass -c with a new file, or set role = \"node\" in this one",
                    path.display()
                );
            }
            Some(doc)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(err).with_context(|| format!("cannot read {}", path.display())),
    };
    let data_dir = base.join(
        doc.as_ref()
            .and_then(|d| d.get("data_dir"))
            .and_then(|d| d.as_str())
            .unwrap_or("data"),
    );

    let joined = watcher_node::join(&url, &token, &data_dir).await?;

    let text = match &mut doc {
        Some(doc) => {
            doc["node"]["hub"] = toml_edit::value(&url);
            doc.to_string()
        }
        None => format!(
            "# Written by `watcher node join`. This machine is a node of the hub below.\n\
             role = \"node\"\n\n[node]\nhub = {}\n",
            toml_edit::value(&url)
        ),
    };
    std::fs::write(&path, text).with_context(|| format!("cannot write {}", path.display()))?;
    let again = if joined.replaced { " again" } else { "" };
    println!("Joined the hub at {url}{again} as node {}.", joined.name);
    println!(
        "Key and certificates: {}",
        watcher_node::files_dir(&data_dir).display()
    );
    let run = match config {
        Some(path) => format!("watcher run {}", path.display()),
        None => "watcher run".into(),
    };
    println!("Start the node with: {run}");
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bootstrap(text: &str) -> Bootstrap {
        let dir = std::env::temp_dir().join(format!("watcher-link-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("watcher.toml");
        std::fs::write(&path, text).unwrap();
        Bootstrap::load(&path).unwrap()
    }

    #[test]
    fn link_urls() {
        let url = |text: &str| link_plan(&bootstrap(text)).unwrap().url;
        assert_eq!(
            url("role = \"hub\"\n[hub]\nurl = \"hub.example.com\"\n"),
            "https://hub.example.com:7443"
        );
        assert_eq!(
            url(
                "role = \"hub\"\npublic_url = \"https://w.example.com/\"\n[hub]\nlisten = \"0.0.0.0:9000\"\n"
            ),
            "https://w.example.com:9000"
        );
        assert_eq!(
            url("role = \"hub\"\n[hub]\nlisten = \"192.0.2.1:7443\"\n"),
            "https://192.0.2.1:7443"
        );
        assert_eq!(
            url("role = \"hub\"\n"),
            format!("https://{}:7443", crate::init_cmd::hostname())
        );
        assert!(link_plan(&bootstrap("role = \"hub\"\n[hub]\nlisten = \"nope\"\n")).is_err());
        assert!(link_plan(&bootstrap("role = \"hub\"\n[hub]\nurl = \"http://h\"\n")).is_err());
    }
}
