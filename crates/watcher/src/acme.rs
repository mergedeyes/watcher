//! HTTPS certificates from an ACME CA such as Let's Encrypt (RFC 8555).
//!
//! The CA checks each name with TLS-ALPN-01 (RFC 8737): it connects to
//! port 443 of the name and asks for the protocol `acme-tls/1`, and the
//! server answers with a certificate made for that challenge. So no other
//! port is needed, but port 443 of each name must reach this server.
//!
//! The certificate, its key and the ACME account live in `data_dir/acme/`.
//! A certificate is ordered when there is none, when the names change, and
//! when a third of its lifetime is left (at most 30 days before it ends).
//! The server picks up a new one without a restart; until the first one
//! arrives it serves a self-signed one.

use anyhow::{Context, Result, anyhow, bail};
use axum_server::tls_rustls::RustlsConfig;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use instant_acme::{
    Account, AccountCredentials, AuthorizationStatus, ChallengeType, ExternalAccountKey,
    Identifier, NewAccount, NewOrder, Order, OrderStatus, RetryPolicy,
};
use rustls::ServerConfig;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};
use watcher_config::Acme as Settings;

/// The protocol a CA asks for when it validates TLS-ALPN-01.
const ACME_TLS: &[u8] = b"acme-tls/1";
/// Renew at most this long before a certificate ends: long-lived ones from
/// internal CAs would otherwise be renewed months early.
const RENEW_BEFORE: Duration = Duration::from_secs(30 * 24 * 3600);
/// Look at least this often whether a renewal is due, in case the clock
/// jumps.
const CHECK_EVERY: Duration = Duration::from_secs(12 * 3600);
/// The wait after a failed order, doubled each time up to `RETRY_MAX`.
/// CAs limit failed validations (Let's Encrypt: 5 per name and hour).
const RETRY_FIRST: Duration = Duration::from_secs(10 * 60);
const RETRY_MAX: Duration = Duration::from_secs(6 * 3600);

/// Gives the CA the challenge certificate of the name it validates, and
/// everyone else the current certificate.
#[derive(Debug)]
struct Resolver {
    current: RwLock<Arc<CertifiedKey>>,
    /// Challenge certificates by name, while an order is validated.
    challenges: RwLock<HashMap<String, Arc<CertifiedKey>>>,
}

impl ResolvesServerCert for Resolver {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let validation = hello
            .alpn()
            .is_some_and(|mut protocols| protocols.any(|p| p == ACME_TLS));
        if validation {
            let name = hello.server_name()?.to_ascii_lowercase();
            return self
                .challenges
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .get(&name)
                .cloned();
        }
        Some(
            self.current
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        )
    }
}

/// The names and the lifetime of a certificate.
#[derive(Debug, Clone, PartialEq)]
struct Info {
    names: Vec<String>,
    not_before: SystemTime,
    not_after: SystemTime,
}

impl Info {
    /// When to order the next certificate: when a third of the lifetime is
    /// left, as Let's Encrypt advises (30 days of 90), but at most
    /// [`RENEW_BEFORE`] before the end.
    fn renew_at(&self) -> SystemTime {
        let lifetime = self
            .not_after
            .duration_since(self.not_before)
            .unwrap_or_default();
        self.not_after - (lifetime / 3).min(RENEW_BEFORE)
    }

    /// Whether to order a new certificate for `domains` at `now`.
    fn due(&self, domains: &[String], now: SystemTime) -> bool {
        let mut wanted = domains.to_vec();
        wanted.sort();
        wanted != self.names || now >= self.renew_at()
    }
}

/// Keeps the certificate of the server fresh; see [`Acme::run`].
pub struct Acme {
    settings: Settings,
    /// `data_dir/acme`.
    dir: PathBuf,
    /// The decoded HMAC key of the external account binding.
    eab_key: Option<Vec<u8>>,
    resolver: Arc<Resolver>,
    /// The certificate being served; `None` while it is the placeholder.
    served: Option<Info>,
}

/// The TLS config for the server and the task that orders its
/// certificates. Start [`Acme::run`] once the server listens: the CA
/// connects to it.
pub fn setup(
    settings: &Settings,
    data_dir: &Path,
    listen: SocketAddr,
) -> Result<(RustlsConfig, Acme)> {
    // Fails harmlessly if a provider is already installed.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let eab_key = settings
        .eab_hmac_key
        .as_deref()
        .map(|key| URL_SAFE_NO_PAD.decode(key.trim().trim_end_matches('=')))
        .transpose()
        .context("[tls.acme] eab_hmac_key: not base64url")?;
    let dir = data_dir.join("acme");
    let stored = match load(&dir) {
        Ok(stored) => stored,
        Err(err) => {
            warn!("ACME: ignoring the stored certificate: {err:#}");
            None
        }
    };
    let (current, served) = match stored {
        Some((key, info)) => (key, Some(info)),
        None => (placeholder(&settings.domains)?, None),
    };
    let resolver = Arc::new(Resolver {
        current: RwLock::new(Arc::new(current)),
        challenges: RwLock::new(HashMap::new()),
    });
    let config = server_config(resolver.clone());
    if listen.port() != 443 {
        info!(
            "ACME: the CA connects to port 443 of {}; it has to reach {listen}",
            settings.domains.join(", ")
        );
    }
    let acme = Acme {
        settings: settings.clone(),
        dir,
        eab_key,
        resolver,
        served,
    };
    Ok((RustlsConfig::from_config(config), acme))
}

/// HTTP/2 and HTTP/1.1 for browsers, `acme-tls/1` for the CA.
fn server_config(resolver: Arc<Resolver>) -> Arc<ServerConfig> {
    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(resolver);
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec(), ACME_TLS.to_vec()];
    Arc::new(config)
}

impl Acme {
    /// Orders certificates when one is due, for as long as the server runs.
    pub async fn run(mut self) {
        let mut retry = RETRY_FIRST;
        loop {
            let now = SystemTime::now();
            if let Some(info) = &self.served
                && !info.due(&self.settings.domains, now)
            {
                let wait = info.renew_at().duration_since(now).unwrap_or_default();
                tokio::time::sleep(wait.min(CHECK_EVERY)).await;
                continue;
            }
            match self.renew().await {
                Ok(info) => {
                    info!(
                        "ACME: new certificate for {}, valid until {}",
                        info.names.join(", "),
                        humantime::format_rfc3339_seconds(info.not_after)
                    );
                    self.served = Some(info);
                    retry = RETRY_FIRST;
                }
                Err(err) => {
                    warn!(
                        "ACME: no certificate from {}: {err:#}; next try in {}",
                        self.settings.directory,
                        humantime::format_duration(retry)
                    );
                    tokio::time::sleep(retry).await;
                    retry = (retry * 2).min(RETRY_MAX);
                }
            }
        }
    }

    /// Orders a certificate, stores it and serves it.
    async fn renew(&self) -> Result<Info> {
        let account = self.account().await?;
        let identifiers: Vec<Identifier> = self
            .settings
            .domains
            .iter()
            .map(|name| Identifier::Dns(name.clone()))
            .collect();
        let mut order = account
            .new_order(&NewOrder::new(&identifiers))
            .await
            .context("new order")?;
        let validated = self.validate(&mut order).await;
        // Challenge certificates are only needed while the CA validates.
        self.resolver
            .challenges
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        validated?;
        let key_pem = order.finalize().await.context("finalize")?;
        let chain_pem = order
            .poll_certificate(&patience())
            .await
            .context("certificate")?;
        let (key, info) = parse(chain_pem.as_bytes(), key_pem.as_bytes())?;
        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("cannot create {}", self.dir.display()))?;
        crate::init_cmd::write_file(&self.dir.join("key.pem"), &key_pem, true)
            .context("cannot store key.pem")?;
        crate::init_cmd::write_file(&self.dir.join("cert.pem"), &chain_pem, false)
            .context("cannot store cert.pem")?;
        *self
            .resolver
            .current
            .write()
            .unwrap_or_else(|e| e.into_inner()) = Arc::new(key);
        Ok(info)
    }

    /// Answers the TLS-ALPN-01 challenge of every name, then waits until
    /// the CA has checked them all.
    async fn validate(&self, order: &mut Order) -> Result<()> {
        let mut authorizations = order.authorizations();
        while let Some(authz) = authorizations.next().await {
            let mut authz = authz.context("authorization")?;
            match authz.status {
                AuthorizationStatus::Pending => {}
                AuthorizationStatus::Valid => continue,
                status => bail!("authorization is {status:?}"),
            }
            let mut challenge = authz
                .challenge(ChallengeType::TlsAlpn01)
                .ok_or_else(|| anyhow!("the CA offers no TLS-ALPN-01 challenge"))?;
            let name = challenge.identifier().to_string().to_ascii_lowercase();
            let cert = challenge_cert(&name, challenge.key_authorization().digest().as_ref())?;
            self.resolver
                .challenges
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .insert(name, Arc::new(cert));
            challenge.set_ready().await.context("challenge")?;
        }
        match order.poll_ready(&patience()).await.context("validation")? {
            OrderStatus::Ready => Ok(()),
            status => bail!("the order is {status:?}"),
        }
    }

    /// The stored account for this CA, or a new one.
    async fn account(&self) -> Result<Account> {
        let file = self.dir.join("account.json");
        let builder = || match &self.settings.ca {
            Some(ca) => Account::builder_with_root(ca)
                .with_context(|| format!("[tls.acme] ca: {}", ca.display())),
            None => Account::builder().context("HTTP client"),
        };
        let stored = std::fs::read_to_string(&file)
            .ok()
            .and_then(|text| serde_json::from_str::<StoredAccount>(&text).ok())
            .filter(|stored| stored.directory == self.settings.directory);
        if let Some(stored) = stored {
            return builder()?
                .from_credentials(stored.credentials)
                .await
                .context("account");
        }
        let contact = self.settings.email.as_ref().map(|e| format!("mailto:{e}"));
        let contacts: Vec<&str> = contact.iter().map(String::as_str).collect();
        let eab = self
            .settings
            .eab_kid
            .clone()
            .zip(self.eab_key.as_deref())
            .map(|(kid, key)| ExternalAccountKey::new(kid, key));
        let (account, credentials) = builder()?
            .create(
                &NewAccount {
                    contact: &contacts,
                    // Setting up ACME agrees to the CA's terms (the docs say so).
                    terms_of_service_agreed: true,
                    only_return_existing: false,
                },
                self.settings.directory.clone(),
                eab.as_ref(),
            )
            .await
            .context("new account")?;
        let stored = StoredAccount {
            directory: self.settings.directory.clone(),
            credentials,
        };
        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("cannot create {}", self.dir.display()))?;
        crate::init_cmd::write_file(&file, &serde_json::to_string_pretty(&stored)?, true)
            .with_context(|| format!("cannot write {}", file.display()))?;
        info!("ACME: created an account at {}", self.settings.directory);
        Ok(account)
    }
}

/// `account.json`: the account's key, with the CA it belongs to, so a
/// change of `directory` makes a new account.
#[derive(Serialize, Deserialize)]
struct StoredAccount {
    directory: String,
    credentials: AccountCredentials,
}

/// Up to two minutes for the CA to validate and to issue.
fn patience() -> RetryPolicy {
    RetryPolicy::new()
        .initial_delay(Duration::from_secs(1))
        .backoff(1.5)
        .timeout(Duration::from_secs(120))
}

/// The certificate in `dir`, if there is one.
fn load(dir: &Path) -> Result<Option<(CertifiedKey, Info)>> {
    let (cert, key) = (dir.join("cert.pem"), dir.join("key.pem"));
    if !cert.exists() || !key.exists() {
        return Ok(None);
    }
    let chain = std::fs::read(&cert).with_context(|| format!("{}", cert.display()))?;
    let key = std::fs::read(&key).with_context(|| format!("{}", key.display()))?;
    parse(&chain, &key).map(Some)
}

/// A PEM chain and key, ready to serve, and the leaf's names and end.
fn parse(chain_pem: &[u8], key_pem: &[u8]) -> Result<(CertifiedKey, Info)> {
    let chain = CertificateDer::pem_slice_iter(chain_pem)
        .collect::<Result<Vec<_>, _>>()
        .context("certificate chain")?;
    let leaf = chain.first().context("certificate chain: empty")?;
    let info = info(leaf)?;
    let key = PrivateKeyDer::from_pem_slice(key_pem).context("private key")?;
    Ok((certified(chain, &key)?, info))
}

fn info(leaf: &CertificateDer<'_>) -> Result<Info> {
    let (_, cert) = x509_parser::parse_x509_certificate(leaf).context("certificate")?;
    let time = |t: i64| UNIX_EPOCH + Duration::from_secs(u64::try_from(t).unwrap_or(0));
    let not_before = time(cert.validity().not_before.timestamp());
    let not_after = time(cert.validity().not_after.timestamp());
    let mut names: Vec<String> = cert
        .subject_alternative_name()
        .ok()
        .flatten()
        .map(|san| {
            san.value
                .general_names
                .iter()
                .filter_map(|name| match name {
                    x509_parser::extensions::GeneralName::DNSName(dns) => {
                        Some(dns.to_ascii_lowercase())
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    Ok(Info {
        names,
        not_before,
        not_after,
    })
}

fn certified(chain: Vec<CertificateDer<'static>>, key: &PrivateKeyDer<'_>) -> Result<CertifiedKey> {
    let signer = rustls::crypto::ring::sign::any_supported_type(key)
        .map_err(|e| anyhow!("private key: {e}"))?;
    Ok(CertifiedKey::new(chain, signer))
}

/// A self-signed certificate for `names`, served until the first one from
/// the CA arrives.
fn placeholder(names: &[String]) -> Result<CertifiedKey> {
    let made = rcgen::generate_simple_self_signed(names.to_vec())
        .context("cannot create a temporary certificate")?;
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(made.signing_key.serialize_der()));
    certified(vec![made.cert.der().clone()], &key)
}

/// The certificate that answers the TLS-ALPN-01 challenge for `name`: it
/// carries the SHA-256 digest of the key authorization (RFC 8737, 3).
fn challenge_cert(name: &str, digest: &[u8]) -> Result<CertifiedKey> {
    let mut params = rcgen::CertificateParams::new(vec![name.to_string()])?;
    params.custom_extensions = vec![rcgen::CustomExtension::new_acme_identifier(digest)];
    let key = rcgen::KeyPair::generate()?;
    let cert = params.self_signed(&key)?;
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
    certified(vec![cert.der().clone()], &key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{ServerName, UnixTime};
    use rustls::{
        ClientConfig, ClientConnection, DigitallySignedStruct, ServerConnection, SignatureScheme,
    };

    const DAY: Duration = Duration::from_secs(24 * 3600);

    #[test]
    fn renews_when_names_change_or_a_third_is_left() {
        let start = SystemTime::now();
        let names = ["b.example".to_string(), "a.example".to_string()];
        let cert = |days: u32| Info {
            names: vec!["a.example".into(), "b.example".into()],
            not_before: start,
            not_after: start + days * DAY,
        };
        // Let's Encrypt: 90 days, renewed with 30 left.
        let info = cert(90);
        assert!(!info.due(&names, start + 59 * DAY));
        assert!(info.due(&names, start + 60 * DAY));
        assert!(info.due(&names, start + 91 * DAY), "expired");
        assert!(info.due(&names[..1], start), "a name less");
        // Short-lived: 6 days, renewed with 2 left.
        assert!(!cert(6).due(&names, start + 3 * DAY));
        assert!(cert(6).due(&names, start + 4 * DAY));
        // A year from an internal CA: 30 days before, not four months.
        assert!(!cert(365).due(&names, start + 300 * DAY));
        assert!(cert(365).due(&names, start + 335 * DAY));
    }

    #[test]
    fn challenge_certificates_carry_the_digest() {
        let digest = [7u8; 32];
        let key = challenge_cert("w.example", &digest).unwrap();
        let (_, cert) = x509_parser::parse_x509_certificate(&key.cert[0]).unwrap();
        // id-pe-acmeIdentifier, critical, an OCTET STRING of the digest.
        let ext = cert
            .extensions()
            .iter()
            .find(|e| e.oid.to_id_string() == "1.3.6.1.5.5.7.1.31")
            .unwrap();
        assert!(ext.critical);
        assert_eq!(&ext.value[2..], &digest);
        assert_eq!(info(&key.cert[0]).unwrap().names, ["w.example"]);
    }

    #[test]
    fn parses_stored_certificates() {
        let made = rcgen::generate_simple_self_signed(vec!["B.example".into(), "a.example".into()])
            .unwrap();
        let (key, info) = parse(
            made.cert.pem().as_bytes(),
            made.signing_key.serialize_pem().as_bytes(),
        )
        .unwrap();
        assert_eq!(key.cert.len(), 1);
        assert_eq!(info.names, ["a.example", "b.example"]);
        assert!(info.not_after > SystemTime::now() + 365 * DAY);
        assert!(parse(b"", made.signing_key.serialize_pem().as_bytes()).is_err());
    }

    /// Accepts any certificate: the test looks at which one it got.
    #[derive(Debug)]
    struct AnyCert;

    impl ServerCertVerifier for AnyCert {
        fn verify_server_cert(
            &self,
            _: &CertificateDer<'_>,
            _: &[CertificateDer<'_>],
            _: &ServerName<'_>,
            _: &[u8],
            _: UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            Ok(ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            _: &[u8],
            _: &CertificateDer<'_>,
            _: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn verify_tls13_signature(
            &self,
            _: &[u8],
            _: &CertificateDer<'_>,
            _: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            rustls::crypto::ring::default_provider()
                .signature_verification_algorithms
                .supported_schemes()
        }
    }

    /// The certificate a client offering `alpn` gets for `name`.
    fn handshake(server: Arc<ServerConfig>, name: &str, alpn: &[u8]) -> Option<Vec<u8>> {
        let mut client = ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AnyCert))
            .with_no_client_auth();
        client.alpn_protocols = vec![alpn.to_vec()];
        let mut client = ClientConnection::new(
            Arc::new(client),
            ServerName::try_from(name.to_string()).unwrap(),
        )
        .unwrap();
        let mut server = ServerConnection::new(server).unwrap();
        for _ in 0..10 {
            let mut buf = Vec::new();
            client.write_tls(&mut buf).unwrap();
            server.read_tls(&mut buf.as_slice()).unwrap();
            if server.process_new_packets().is_err() {
                return None;
            }
            let mut buf = Vec::new();
            server.write_tls(&mut buf).unwrap();
            client.read_tls(&mut buf.as_slice()).unwrap();
            if client.process_new_packets().is_err() {
                return None;
            }
            if !client.is_handshaking() {
                return Some(client.peer_certificates()?[0].to_vec());
            }
        }
        None
    }

    #[test]
    fn the_ca_gets_the_challenge_and_browsers_the_certificate() {
        let settings = Settings {
            domains: vec!["w.example".into()],
            email: None,
            directory: watcher_config::LETS_ENCRYPT.into(),
            ca: None,
            eab_kid: None,
            eab_hmac_key: None,
        };
        let dir = std::env::temp_dir().join(format!("watcher-acme-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (_, acme) = setup(&settings, &dir, "127.0.0.1:443".parse().unwrap()).unwrap();
        assert!(acme.served.is_none(), "nothing stored yet");
        let current = acme.resolver.current.read().unwrap().cert[0].to_vec();
        let challenge = challenge_cert("w.example", &[1u8; 32]).unwrap();
        let challenge_der = challenge.cert[0].to_vec();
        acme.resolver
            .challenges
            .write()
            .unwrap()
            .insert("w.example".into(), Arc::new(challenge));
        let config = server_config(acme.resolver.clone());

        assert_eq!(
            handshake(config.clone(), "w.example", b"h2"),
            Some(current.clone())
        );
        assert_eq!(
            handshake(config.clone(), "W.Example", ACME_TLS),
            Some(challenge_der)
        );
        assert_eq!(handshake(config.clone(), "other.example", ACME_TLS), None);
        assert_eq!(
            handshake(config, "other.example", b"http/1.1"),
            Some(current)
        );
    }
}
