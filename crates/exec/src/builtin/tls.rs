//! `tls`: connect with TLS and check the server's certificate: trusted,
//! valid for the name, and how many days until it expires. Like
//! `check_ssl_cert` or `check_http -C`.

use super::{Vars, chain, host_part, label, took};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, ClientConfig, DigitallySignedStruct, SignatureScheme};
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tls {
    pub address: String,
    pub port: u16,
    /// The name the certificate must be valid for, also sent as SNI.
    pub name: String,
    /// WARNING and CRITICAL at this many days before expiry.
    pub warn: i64,
    pub crit: i64,
    /// Whether an untrusted certificate is CRITICAL.
    pub verify: bool,
}

impl Tls {
    /// Vars: `port` (443), `name` (the host's address), `warn` (30 days),
    /// `crit` (14 days), `verify` (true).
    pub(super) fn new(address: &str, vars: &Vars<'_>) -> Result<Self, String> {
        let name = vars.get("name").unwrap_or(address).to_string();
        ServerName::try_from(name.clone())
            .map_err(|_| format!("vars.name {name:?}: not a host name or address"))?;
        let warn = vars.number("warn", 30)?;
        let crit = vars.number("crit", 14)?;
        Ok(Tls {
            address: address.to_string(),
            port: vars.port("port", Some(443))?,
            name,
            warn,
            crit,
            verify: vars.flag("verify", true)?,
        })
    }

    pub(super) async fn run(&self, wait: Duration) -> (i32, String) {
        let target = format!("{}:{}", host_part(&self.address), self.port);
        let verifier = match platform_verifier() {
            Ok(v) => v,
            Err(e) => return (3, format!("TLS UNKNOWN - no certificate verifier: {e}")),
        };
        let recording = Arc::new(Recording {
            inner: verifier,
            error: Mutex::new(None),
        });
        let config = match ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
        {
            Ok(b) => b
                .dangerous()
                .with_custom_certificate_verifier(recording.clone())
                .with_no_client_auth(),
            Err(e) => return (3, format!("TLS UNKNOWN - {e}")),
        };
        let server_name = match ServerName::try_from(self.name.clone()) {
            Ok(n) => n,
            Err(e) => return (3, format!("TLS UNKNOWN - {}: {e}", self.name)),
        };
        let handshake = async {
            let tcp = TcpStream::connect((self.address.as_str(), self.port))
                .await
                .map_err(|e| e.to_string())?;
            TlsConnector::from(Arc::new(config))
                .connect(server_name, tcp)
                .await
                .map_err(|e| format!("handshake: {}", chain(&e)))
        };
        let stream = match tokio::time::timeout(wait, handshake).await {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => return (2, format!("TLS CRITICAL - {target}: {e}")),
            Err(_) => {
                return (
                    2,
                    format!(
                        "TLS CRITICAL - {target}: no handshake within {}",
                        took(wait)
                    ),
                );
            }
        };
        let Some(der) = stream
            .get_ref()
            .1
            .peer_certificates()
            .and_then(|c| c.first())
        else {
            return (2, format!("TLS CRITICAL - {target}: no certificate"));
        };
        let cert = match x509_parser::parse_x509_certificate(der) {
            Ok((_, cert)) => cert,
            Err(e) => {
                return (
                    3,
                    format!("TLS UNKNOWN - {target}: cannot read the certificate: {e}"),
                );
            }
        };
        let validity = cert.validity();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let not_after = validity.not_after.timestamp();
        let days = (not_after - now).div_euclid(86_400);
        let until = validity.not_after.to_datetime().date();
        let issuer = cert
            .issuer()
            .iter_common_name()
            .chain(cert.issuer().iter_organization())
            .find_map(|a| a.as_str().ok())
            .unwrap_or("unknown issuer")
            .to_string();
        let untrusted = recording
            .error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .filter(|e| !dates_only(e));

        let (mut code, mut text) = if not_after <= now {
            (2, format!("expired {} days ago ({until})", -days))
        } else if validity.not_before.timestamp() > now {
            (
                2,
                format!(
                    "not valid before {}",
                    validity.not_before.to_datetime().date()
                ),
            )
        } else {
            let code = if days <= self.crit {
                2
            } else if days <= self.warn {
                1
            } else {
                0
            };
            (code, format!("valid until {until} ({days} days)"))
        };
        text = match untrusted {
            Some(e) => {
                if self.verify {
                    code = 2;
                }
                let why = untrusted_text(&e);
                format!(
                    "{}: not trusted ({why}) - {text}, issued by {issuer}",
                    self.name
                )
            }
            None => format!("{}: {text}, issued by {issuer}", self.name),
        };
        (
            code,
            format!(
                "TLS {} - {text}|days={days};{};{};0",
                label(code),
                self.warn,
                self.crit
            ),
        )
    }
}

impl fmt::Display for Tls {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "tls {}:{}", host_part(&self.address), self.port)?;
        if self.name != self.address {
            write!(f, " name={}", self.name)?;
        }
        write!(f, " warn={}d crit={}d", self.warn, self.crit)?;
        if !self.verify {
            f.write_str(" verify=false")?;
        }
        Ok(())
    }
}

pub(super) fn provider() -> Arc<CryptoProvider> {
    static PROVIDER: OnceLock<Arc<CryptoProvider>> = OnceLock::new();
    PROVIDER
        .get_or_init(|| Arc::new(rustls::crypto::ring::default_provider()))
        .clone()
}

/// The system's trust store (as reqwest uses), shared by all runs.
pub(super) fn platform_verifier() -> Result<Arc<dyn ServerCertVerifier>, String> {
    static VERIFIER: OnceLock<Result<Arc<dyn ServerCertVerifier>, String>> = OnceLock::new();
    VERIFIER
        .get_or_init(|| {
            rustls_platform_verifier::Verifier::new(provider())
                .map(|v| Arc::new(v) as Arc<dyn ServerCertVerifier>)
                .map_err(|e| e.to_string())
        })
        .clone()
}

/// Errors about the dates only: the check reports those itself.
fn dates_only(e: &rustls::Error) -> bool {
    matches!(
        e,
        rustls::Error::InvalidCertificate(
            CertificateError::Expired
                | CertificateError::ExpiredContext { .. }
                | CertificateError::NotValidYet
                | CertificateError::NotValidYetContext { .. }
        )
    )
}

fn untrusted_text(e: &rustls::Error) -> String {
    match e {
        rustls::Error::InvalidCertificate(CertificateError::UnknownIssuer) => {
            "unknown issuer".into()
        }
        rustls::Error::InvalidCertificate(
            CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. },
        ) => "not valid for this name".into(),
        rustls::Error::InvalidCertificate(CertificateError::Revoked) => "revoked".into(),
        other => other.to_string(),
    }
}

/// Verifies like `inner`, but keeps the error and lets the handshake go
/// on, so the certificate can still be read and reported.
#[derive(Debug)]
struct Recording {
    inner: Arc<dyn ServerCertVerifier>,
    error: Mutex<Option<rustls::Error>>,
}

impl ServerCertVerifier for Recording {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if let Err(e) = self.inner.verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            ocsp_response,
            now,
        ) {
            *self.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(e);
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::{Builtin, vars};
    use rustls::ServerConfig;
    use rustls::pki_types::PrivateKeyDer;
    use tokio_rustls::TlsAcceptor;

    /// Civil date `days` from today, for certificate dates.
    fn date(days: i64) -> (i32, u8, u8) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        // Howard Hinnant's days-to-civil.
        let z = now.div_euclid(86_400) + days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + i64::from(m <= 2);
        (y as i32, m as u8, d as u8)
    }

    /// A TLS server for `localhost` with a self-signed certificate that
    /// started `from` days ago and expires in `to` days.
    async fn server(from: i64, to: i64) -> String {
        let key = rcgen::KeyPair::generate().unwrap();
        let mut params = rcgen::CertificateParams::new(vec!["localhost".into()]).unwrap();
        let (y, m, d) = date(-from);
        params.not_before = rcgen::date_time_ymd(y, m, d);
        let (y, m, d) = date(to);
        params.not_after = rcgen::date_time_ymd(y, m, d);
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "Test CA");
        let cert = params.self_signed(&key).unwrap();
        let config = ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![cert.der().clone()],
                PrivateKeyDer::Pkcs8(key.serialize_der().into()),
            )
            .unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (s, _) = listener.accept().await.unwrap();
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    if let Ok(mut tls) = acceptor.accept(s).await {
                        use tokio::io::AsyncReadExt;
                        let mut buf = [0u8; 16];
                        let _ = tls.read(&mut buf).await;
                    }
                });
            }
        });
        port.to_string()
    }

    async fn run(port: &str, extra: &[(&str, &str)]) -> (i32, String) {
        let mut list = vec![("port", port), ("name", "localhost")];
        list.extend_from_slice(extra);
        Builtin::new("tls", "127.0.0.1", &vars(&list))
            .unwrap()
            .run(Duration::from_secs(3), None)
            .await
    }

    #[test]
    fn settings() {
        let b = Builtin::new(
            "tls",
            "10.0.0.5",
            &vars(&[("name", "www.example.com"), ("warn", "20")]),
        )
        .unwrap();
        assert_eq!(
            b.to_string(),
            "tls 10.0.0.5:443 name=www.example.com warn=20d crit=14d (built-in)"
        );
        assert!(Builtin::new("tls", "h", &vars(&[("warn", "soon")])).is_err());
        assert!(Builtin::new("tls", "h", &vars(&[("name", "bad name")])).is_err());
    }

    #[tokio::test]
    async fn expiry_days() {
        let port = server(1, 90).await;
        let (code, out) = run(&port, &[("verify", "false")]).await;
        assert_eq!(code, 0, "{out}");
        assert!(
            out.starts_with("TLS OK - localhost: not trusted (unknown issuer) - valid until "),
            "{out}"
        );
        assert!(
            out.contains("(89 days), issued by Test CA|days=89;30;14;0"),
            "{out}"
        );

        let port = server(1, 20).await;
        let (code, out) = run(&port, &[("verify", "false")]).await;
        assert_eq!(code, 1, "{out}");
        let port = server(1, 5).await;
        assert_eq!(run(&port, &[("verify", "false")]).await.0, 2);

        let port = server(30, -3).await;
        let (code, out) = run(&port, &[("verify", "false")]).await;
        assert_eq!(code, 2, "{out}");
        assert!(
            out.contains("expired 4 days ago") || out.contains("expired 3 days ago"),
            "{out}"
        );
    }

    #[tokio::test]
    async fn untrusted_is_critical() {
        let port = server(1, 90).await;
        let (code, out) = run(&port, &[]).await;
        assert_eq!(code, 2, "{out}");
        assert!(
            out.starts_with("TLS CRITICAL - localhost: not trusted (unknown issuer)"),
            "{out}"
        );

        // Not TLS at all.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port().to_string();
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                let _ = s.write_all(b"SSH-2.0-Test\r\n").await;
            }
        });
        let (code, out) = run(&port, &[]).await;
        assert_eq!(code, 2, "{out}");
        assert!(out.contains("handshake"), "{out}");
    }
}
