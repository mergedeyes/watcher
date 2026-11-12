//! TLS of the node link, done here rather than by tonic: joining trusts
//! the hub through the CA fingerprint in the join token, which tonic's
//! settings can't express.

use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use std::sync::Arc;
use watcher_proto::fingerprint;

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn finish(config: ClientConfig) -> Arc<ClientConfig> {
    let mut config = config;
    // gRPC needs HTTP/2.
    config.alpn_protocols = vec![b"h2".to_vec()];
    Arc::new(config)
}

/// For joining: trusts the hub if its chain holds the CA with fingerprint
/// `ca` and that CA issued the hub's certificate.
pub fn join(ca: &str) -> Result<Arc<ClientConfig>, rustls::Error> {
    let provider = provider();
    let verifier = PinnedCa {
        fingerprint: ca.to_string(),
        algorithms: provider.signature_verification_algorithms,
        provider: provider.clone(),
    };
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    Ok(finish(config))
}

/// For sessions: trusts only `ca` and shows the node's certificate.
pub fn session(
    ca: CertificateDer<'static>,
    cert: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
) -> Result<Arc<ClientConfig>, rustls::Error> {
    let mut roots = RootCertStore::empty();
    roots.add(ca)?;
    let config = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_client_auth_cert(vec![cert], key)?;
    Ok(finish(config))
}

#[derive(Debug)]
struct PinnedCa {
    fingerprint: String,
    algorithms: WebPkiSupportedAlgorithms,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinnedCa {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let ca = intermediates
            .iter()
            .find(|c| fingerprint(c) == self.fingerprint)
            .ok_or_else(|| {
                rustls::Error::General(
                    "the hub's certificate is not from the CA the join token names \
                     (is this the right hub, and the whole token?)"
                        .into(),
                )
            })?;
        let mut roots = RootCertStore::empty();
        roots.add(ca.clone().into_owned())?;
        WebPkiServerVerifier::builder_with_provider(Arc::new(roots), self.provider.clone())
            .build()
            .map_err(|e| rustls::Error::General(e.to_string()))?
            .verify_server_cert(end_entity, &[], server_name, ocsp_response, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}
