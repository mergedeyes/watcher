//! The node: joins a hub once, then keeps a session with it open over
//! mutual TLS, and connects again whenever the link drops.
//!
//! Its key and certificates live in `data_dir/node/`. They are read again
//! for every connection, so joining again takes effect without a restart.

mod tls;

use anyhow::{Context, Result, anyhow, bail};
use hyper_util::rt::TokioIo;
use rcgen::{CertificateParams, KeyPair};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_rustls::TlsConnector;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::{Channel, Endpoint, Uri};
use tonic::{Code, Status};
use tracing::{info, warn};
use watcher_proto::link_client::LinkClient;
use watcher_proto::{
    HUB_NAME, Hello, JoinRequest, JoinToken, NodeMessage, fingerprint, hub_address, hub_message,
    hub_url, node_message,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The node's files, below `data_dir`.
const DIR: &str = "node";
const KEY: &str = "key.pem";
const CERT: &str = "cert.pem";
const CA: &str = "ca.pem";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// HTTP/2 pings find connections that died without closing (NAT, power
/// loss) within about a minute.
const KEEPALIVE: Duration = Duration::from_secs(30);
const KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(20);
/// Waits between connection attempts: doubling from the first to the last.
const FIRST_RETRY: Duration = Duration::from_secs(2);
const LAST_RETRY: Duration = Duration::from_secs(60);

/// Where the node keeps its key and certificates.
pub fn files_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(DIR)
}

/// Whether this machine has joined a hub (has a certificate).
pub fn joined(data_dir: &Path) -> bool {
    files_dir(data_dir).join(CERT).exists()
}

/// A connection to the hub at `host:port`, with TLS from `tls`.
async fn channel(host: &str, port: u16, tls: Arc<rustls::ClientConfig>) -> Result<Channel> {
    // `http`, since the connector below does TLS itself; tonic refuses
    // `https` without its own TLS settings.
    let uri = format!("http://{}", &hub_url(host, port)["https://".len()..]);
    let endpoint = Endpoint::from_shared(uri)?
        .connect_timeout(CONNECT_TIMEOUT)
        .http2_keep_alive_interval(KEEPALIVE)
        .keep_alive_timeout(KEEPALIVE_TIMEOUT)
        .keep_alive_while_idle(true);
    let host = host.to_string();
    let connector = tower::service_fn(move |_: Uri| {
        let (host, tls) = (host.clone(), tls.clone());
        async move {
            let tcp = TcpStream::connect((host.as_str(), port)).await?;
            tcp.set_nodelay(true)?;
            // The hub's certificate is for HUB_NAME, whatever address
            // reaches it.
            let name = ServerName::try_from(HUB_NAME).expect("a valid name");
            let stream = TlsConnector::from(tls).connect(name, tcp).await?;
            Ok::<_, std::io::Error>(TokioIo::new(stream))
        }
    });
    endpoint
        .connect_with_connector(connector)
        .await
        .map_err(|err| {
            // tonic and hyper wrap the cause and repeat it; say it once.
            let mut cause: &dyn std::error::Error = &err;
            while let Some(source) = cause.source() {
                cause = source;
            }
            anyhow!("{cause}")
        })
}

/// What the hub's refusal means for this node.
fn refused(status: Status) -> anyhow::Error {
    match status.code() {
        Code::PermissionDenied | Code::Unauthenticated => {
            anyhow!("the hub refused this node: {}", status.message())
        }
        _ => anyhow!("{}", status.message()),
    }
}

/// Why a session that was under way ended.
fn ended(status: Status, url: &str) -> anyhow::Error {
    match status.code() {
        Code::Aborted => anyhow!("the hub ended the connection: {}", status.message()),
        Code::PermissionDenied | Code::Unauthenticated => refused(status),
        _ => anyhow!(
            "lost the connection to the hub at {url} ({})",
            status.message()
        ),
    }
}

/// The node's name on the hub, once it has joined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Joined {
    pub name: String,
    /// The machine had joined before; that certificate was replaced.
    pub replaced: bool,
}

/// Joins the hub at `url` with `token`: makes a key, has the hub sign a
/// certificate for it, and keeps both and the hub's CA in `data_dir/node/`.
pub async fn join(url: &str, token: &JoinToken, data_dir: &Path) -> Result<Joined> {
    let (host, port) = hub_address(url).map_err(|e| anyhow!(e))?;
    let key = KeyPair::generate()?;
    let csr = CertificateParams::default().serialize_request(&key)?;
    let channel = channel(&host, port, tls::join(&token.ca)?)
        .await
        .with_context(|| format!("cannot connect to the hub at {}", hub_url(&host, port)))?;
    let reply = LinkClient::new(channel)
        .join(JoinRequest {
            token: token.secret.clone(),
            csr: csr.der().to_vec(),
            version: VERSION.into(),
        })
        .await
        .map_err(refused)?
        .into_inner();
    let ca = CertificateDer::from_pem_slice(reply.ca.as_bytes()).context("the hub's CA")?;
    if fingerprint(&ca) != token.ca {
        bail!("the hub sent a CA other than the one the join token names");
    }
    CertificateDer::from_pem_slice(reply.certificate.as_bytes())
        .context("the node certificate from the hub")?;

    let dir = files_dir(data_dir);
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let replaced = joined(data_dir);
    // The certificate goes last: it is what marks the node as joined.
    write(&dir.join(KEY), &key.serialize_pem(), true)?;
    write(&dir.join(CA), &reply.ca, false)?;
    write(&dir.join(CERT), &reply.certificate, false)?;
    Ok(Joined {
        name: reply.name,
        replaced,
    })
}

/// Writes `text` to `path` through a temporary file; with `private`,
/// readable by the owner only.
fn write(path: &Path, text: &str, private: bool) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    if private {
        options.mode(0o600);
    }
    options
        .open(&tmp)
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .and_then(|()| std::fs::rename(&tmp, path))
        .with_context(|| format!("cannot write {}", path.display()))
}

/// The TLS settings for a session, from the files of `join`.
fn session_tls(data_dir: &Path) -> Result<Arc<rustls::ClientConfig>> {
    let dir = files_dir(data_dir);
    if !joined(data_dir) {
        bail!(
            "this machine has not joined a hub yet (no {}); run: watcher node join <hub> --token <token>",
            dir.join(CERT).display()
        );
    }
    let path = |name: &str| dir.join(name);
    let ca = CertificateDer::from_pem_file(path(CA))
        .with_context(|| format!("cannot read {}", path(CA).display()))?;
    let cert = CertificateDer::from_pem_file(path(CERT))
        .with_context(|| format!("cannot read {}", path(CERT).display()))?;
    let key = PrivateKeyDer::from_pem_file(path(KEY))
        .with_context(|| format!("cannot read {}", path(KEY).display()))?;
    Ok(tls::session(ca, cert, key)?)
}

/// One session with the hub, until either side ends it. Sets `connected`
/// once the hub has welcomed the node.
async fn session(
    host: &str,
    port: u16,
    data_dir: &Path,
    hostname: &str,
    connected: &mut bool,
) -> Result<()> {
    let tls = session_tls(data_dir)?;
    let url = hub_url(host, port);
    let channel = channel(host, port, tls)
        .await
        .with_context(|| format!("cannot connect to the hub at {url}"))?;
    let (tx, rx) = mpsc::channel(16);
    let hello = NodeMessage {
        kind: Some(node_message::Kind::Hello(Hello {
            version: VERSION.into(),
            hostname: hostname.into(),
        })),
    };
    tx.send(hello).await.expect("the receiver is still here");
    let mut inbound = LinkClient::new(channel)
        .session(ReceiverStream::new(rx))
        .await
        .map_err(refused)?
        .into_inner();
    match inbound.message().await.map_err(refused)? {
        Some(message) => match message.kind {
            Some(hub_message::Kind::Welcome(welcome)) => {
                *connected = true;
                info!(
                    "connected to the hub at {url} as node {} (hub version {})",
                    welcome.name, welcome.version
                );
            }
            None => bail!("the hub answered with a message this version doesn't know"),
        },
        None => bail!("the hub closed the connection right away"),
    }
    // The hub sends nothing else yet; the session lasts until it ends.
    while inbound
        .message()
        .await
        .map_err(|status| ended(status, &url))?
        .is_some()
    {}
    drop(tx);
    Ok(())
}

/// Keeps a session with the hub at `url` open until `shutdown` completes,
/// connecting again after each drop. Fails only if this machine hasn't
/// joined a hub.
pub async fn run(
    url: &str,
    data_dir: &Path,
    hostname: &str,
    shutdown: impl Future<Output = ()>,
) -> Result<()> {
    let (host, port) = hub_address(url).map_err(|e| anyhow!(e))?;
    session_tls(data_dir)?;
    tokio::pin!(shutdown);
    let mut wait = FIRST_RETRY;
    loop {
        let mut connected = false;
        let result = tokio::select! {
            result = session(&host, port, data_dir, hostname, &mut connected) => result,
            () = &mut shutdown => return Ok(()),
        };
        match result {
            Ok(()) => info!("the hub closed the connection"),
            Err(err) => warn!("{err:#}"),
        }
        // After a session, the first retry comes soon again.
        if connected {
            wait = FIRST_RETRY;
        }
        info!("connecting again in {}s", wait.as_secs());
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            () = &mut shutdown => return Ok(()),
        }
        wait = (wait * 2).min(LAST_RETRY);
    }
}
