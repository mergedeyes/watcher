//! The node link on the hub: a gRPC server with mutual TLS. Nodes join
//! there with a token, then keep a session open with their certificate.

use crate::Hub;
use crate::nodes::NodeLink;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_stream::Stream;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::server::TcpIncoming;
use tonic::transport::{Certificate, Identity, Server, ServerTlsConfig};
use tonic::{Request, Response, Status, Streaming};
use tracing::{info, warn};
use watcher_proto::link_server::{Link, LinkServer};
use watcher_proto::{
    HubMessage, JoinReply, JoinRequest, JoinToken, NodeMessage, Welcome, fingerprint, hub_message,
    node_message,
};
use watcher_store::Store;

/// How long a node has to say Hello after connecting.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
/// HTTP/2 pings find connections that died without closing (NAT, power
/// loss) within about a minute.
const KEEPALIVE: Duration = Duration::from_secs(30);
const KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(20);
/// How often sessions are checked against the database, so that
/// `watcher node remove` ends a connection, and `last_seen` is updated.
const SWEEP: Duration = Duration::from_secs(60);

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Serves the node link on `listener`. Needs [`Hub::enable_nodes`] and a
/// database.
pub fn serve(hub: Arc<Hub>, listener: std::net::TcpListener) -> Result<JoinHandle<()>, String> {
    let link: &NodeLink = hub
        .link
        .get()
        .ok_or("the node link needs Hub::enable_nodes")?;
    let store = hub.store.clone().ok_or("nodes need a database")?;
    let (chain, key) = link.ca.server_identity()?;
    let tls = ServerTlsConfig::new()
        .identity(Identity::from_pem(chain, key))
        .client_ca_root(Certificate::from_pem(link.ca.cert_pem()))
        // Joining nodes have no certificate yet; sessions check for one.
        .client_auth_optional(true);
    let listener = listener
        .set_nonblocking(true)
        .and_then(|()| tokio::net::TcpListener::from_std(listener))
        .map_err(|e| format!("node link: {e}"))?;
    let server = Server::builder()
        .tls_config(tls)
        .map_err(|e| format!("node link TLS: {e}"))?
        .http2_keepalive_interval(Some(KEEPALIVE))
        .http2_keepalive_timeout(Some(KEEPALIVE_TIMEOUT))
        .add_service(LinkServer::new(Service {
            hub: hub.clone(),
            store: store.clone(),
        }));
    let incoming = TcpIncoming::from(listener).with_nodelay(Some(true));
    tokio::spawn(sweep(hub, store));
    Ok(tokio::spawn(async move {
        if let Err(err) = server.serve_with_incoming(incoming).await {
            warn!("node link stopped: {err}");
        }
    }))
}

/// Ends sessions of nodes removed outside this process, and updates
/// `last_seen` of the connected ones.
async fn sweep(hub: Arc<Hub>, store: Arc<Store>) {
    let mut every = tokio::time::interval(SWEEP);
    every.tick().await;
    loop {
        every.tick().await;
        let nodes = match store.nodes().await {
            Ok(nodes) => nodes,
            Err(err) => {
                warn!("cannot check the connected nodes: {err}");
                continue;
            }
        };
        let valid: HashMap<String, String> = nodes
            .into_iter()
            .filter_map(|n| Some((n.name, n.cert_fp?)))
            .collect();
        // The sessions log their end themselves.
        hub.nodes.close_revoked(&valid);
        let connected = hub.nodes.connected();
        let names: Vec<&str> = connected.keys().map(String::as_str).collect();
        if let Err(err) = store.nodes_seen(&names).await {
            warn!("cannot record when nodes were last seen: {err}");
        }
    }
}

struct Service {
    hub: Arc<Hub>,
    store: Arc<Store>,
}

fn internal(err: watcher_store::Error) -> Status {
    warn!("node link: {err}");
    Status::internal("the hub cannot reach its database")
}

/// `host` or `host:port` of a peer, for logs.
fn peer(addr: Option<SocketAddr>) -> String {
    addr.map_or("an unknown address".into(), |a| a.to_string())
}

type HubStream = Pin<Box<dyn Stream<Item = Result<HubMessage, Status>> + Send>>;

#[tonic::async_trait]
impl Link for Service {
    async fn join(&self, request: Request<JoinRequest>) -> Result<Response<JoinReply>, Status> {
        let from = peer(request.remote_addr());
        let request = request.into_inner();
        let hash = JoinToken::hash(&request.token);
        let refused = || {
            warn!("refused a node joining from {from}: unknown, used or expired join token");
            Status::permission_denied(
                "the join token is unknown, used or expired; get a new one on the hub",
            )
        };
        let Some(name) = self.store.node_for_token(&hash).await.map_err(internal)? else {
            return Err(refused());
        };
        let ca = &self.hub.link.get().expect("serve checked it").ca;
        let cert = ca
            .sign_node(&name, &request.csr)
            .map_err(Status::invalid_argument)?;
        // The token is used up here; a second join with it fails.
        if !self
            .store
            .join_node(&name, &hash, &cert.fingerprint)
            .await
            .map_err(internal)?
        {
            return Err(refused());
        }
        // A node that joins again: its old certificate no longer works.
        self.hub.nodes.close(&name, "the node joined again");
        info!(
            "node {name} joined from {from} (version {})",
            request.version
        );
        Ok(Response::new(JoinReply {
            name,
            certificate: cert.pem,
            ca: ca.cert_pem().to_string(),
        }))
    }

    type SessionStream = HubStream;

    async fn session(
        &self,
        request: Request<Streaming<NodeMessage>>,
    ) -> Result<Response<HubStream>, Status> {
        let address = request.remote_addr();
        let cert_fp = request
            .peer_certs()
            .and_then(|certs| certs.first().map(|c| fingerprint(c)))
            .ok_or_else(|| Status::unauthenticated("no node certificate; join the hub first"))?;
        let Some(name) = self.store.node_by_cert(&cert_fp).await.map_err(internal)? else {
            warn!(
                "refused a node from {}: its certificate is no longer valid",
                peer(address)
            );
            return Err(Status::permission_denied(
                "this node's certificate is no longer valid: it was removed on the hub, \
                 or joined again elsewhere; join it again",
            ));
        };
        let mut inbound = request.into_inner();
        let hello = match tokio::time::timeout(HELLO_TIMEOUT, inbound.message()).await {
            Ok(Ok(Some(NodeMessage {
                kind: Some(node_message::Kind::Hello(hello)),
            }))) => hello,
            _ => return Err(Status::invalid_argument("expected Hello first")),
        };
        let ip = address.map(|a| a.ip().to_string());
        self.store
            .node_connected(&name, &hello.version, &hello.hostname, ip.as_deref())
            .await
            .map_err(internal)?;

        let (tx, rx) = mpsc::channel(16);
        let welcome = HubMessage {
            kind: Some(hub_message::Kind::Welcome(Welcome {
                name: name.clone(),
                version: VERSION.into(),
            })),
        };
        let _ = tx.send(Ok(welcome)).await;
        let (id, closed) = self.hub.nodes.open(&name, &cert_fp);
        info!(
            "node {name} connected from {} (version {}, host {})",
            peer(address),
            hello.version,
            hello.hostname
        );
        let (hub, store) = (self.hub.clone(), self.store.clone());
        tokio::spawn(async move {
            // The session lasts until the node goes or the hub ends it; the
            // node's messages carry nothing else yet.
            let why = tokio::select! {
                why = closed => why.ok(),
                _ = async { while let Ok(Some(_)) = inbound.message().await {} } => None,
            };
            match why {
                Some(why) => {
                    info!("closed the connection of node {name}: {why}");
                    let _ = tx.send(Err(Status::aborted(why))).await;
                }
                None => info!("node {name} disconnected"),
            }
            hub.nodes.ended(&name, id);
            if let Err(err) = store.nodes_seen(&[&name]).await {
                warn!("cannot record when node {name} was last seen: {err}");
            }
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }
}
