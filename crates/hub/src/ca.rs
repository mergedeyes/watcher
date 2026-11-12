//! The hub's own certificate authority for the node link. It signs the
//! hub's server certificate and each node's client certificate. Nodes trust
//! only this CA; its fingerprint comes with their join token.
//!
//! The certificates keep rcgen's default validity, which runs to the year
//! 4096: the hub knows each node by the fingerprint of its certificate, so
//! removing a node revokes it, and nothing ever needs renewing.

use rcgen::{
    BasicConstraints, CertificateParams, CertificateSigningRequestParams, DnType,
    ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, KeyUsagePurpose,
};
use rustls_pki_types::CertificateDer;
use rustls_pki_types::pem::PemObject;
use std::io::Write;
use std::path::{Path, PathBuf};
use watcher_proto::{HUB_NAME, fingerprint};

/// The CA's files, below `data_dir`.
const DIR: &str = "ca";
const CERT: &str = "ca.pem";
const KEY: &str = "ca.key";

pub struct Ca {
    cert_pem: String,
    issuer: Issuer<'static, KeyPair>,
    fingerprint: String,
}

/// A certificate issued for a node.
pub struct NodeCert {
    pub pem: String,
    /// [`fingerprint`] of the certificate, how the hub knows the node.
    pub fingerprint: String,
}

impl Ca {
    /// The CA in `data_dir/ca/`, created on first use.
    pub fn open_or_create(data_dir: &Path) -> Result<Ca, String> {
        let dir = data_dir.join(DIR);
        if !dir.join(CERT).exists() {
            create(&dir)?;
        }
        let read = |name: &str| {
            let path = dir.join(name);
            std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read {}: {e}", path.display()))
        };
        let (cert_pem, key_pem) = (read(CERT)?, read(KEY)?);
        let bad = |e: &dyn std::fmt::Display| format!("{}: {e}", dir.display());
        let key = KeyPair::from_pem(&key_pem).map_err(|e| bad(&e))?;
        let der = CertificateDer::from_pem_slice(cert_pem.as_bytes()).map_err(|e| bad(&e))?;
        let issuer = Issuer::from_ca_cert_der(&der, key).map_err(|e| bad(&e))?;
        Ok(Ca {
            fingerprint: fingerprint(&der),
            cert_pem,
            issuer,
        })
    }

    /// The CA certificate, PEM.
    pub fn cert_pem(&self) -> &str {
        &self.cert_pem
    }

    /// [`fingerprint`] of the CA certificate; part of every join token.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// A new server certificate for the hub, for [`HUB_NAME`]: the chain
    /// (the certificate, then the CA's, so a joining node finds the CA its
    /// token names) and the key, PEM. Made at every start; nothing keeps it.
    pub fn server_identity(&self) -> Result<(String, String), String> {
        let key = KeyPair::generate().map_err(|e| e.to_string())?;
        let mut params =
            CertificateParams::new(vec![HUB_NAME.to_string()]).map_err(|e| e.to_string())?;
        params.distinguished_name.push(DnType::CommonName, HUB_NAME);
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.use_authority_key_identifier_extension = true;
        let cert = params
            .signed_by(&key, &self.issuer)
            .map_err(|e| e.to_string())?;
        Ok((
            format!("{}{}", cert.pem(), self.cert_pem),
            key.serialize_pem(),
        ))
    }

    /// Signs the certificate signing request `csr` (DER) of node `name`.
    /// Only the request's key is used; the hub decides everything else.
    pub fn sign_node(&self, name: &str, csr: &[u8]) -> Result<NodeCert, String> {
        let csr = CertificateSigningRequestParams::from_der(&csr.into())
            .map_err(|e| format!("invalid certificate signing request: {e}"))?;
        let mut params = CertificateParams::default();
        params.distinguished_name.push(DnType::CommonName, name);
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        params.use_authority_key_identifier_extension = true;
        let cert = params
            .signed_by(&csr.public_key, &self.issuer)
            .map_err(|e| e.to_string())?;
        Ok(NodeCert {
            fingerprint: fingerprint(cert.der()),
            pem: cert.pem(),
        })
    }
}

/// Creates the CA's key and certificate in `dir`. The certificate is
/// written last, so its presence means the CA is complete.
fn create(dir: &Path) -> Result<(), String> {
    let key = KeyPair::generate().map_err(|e| e.to_string())?;
    let mut params = CertificateParams::default();
    params
        .distinguished_name
        .push(DnType::CommonName, "Watcher node CA");
    // It signs only end-entity certificates.
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let cert = params.self_signed(&key).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    write(&dir.join(KEY), &key.serialize_pem(), true)?;
    let tmp = dir.join(format!("{CERT}.tmp"));
    write(&tmp, &cert.pem(), false)?;
    let path = dir.join(CERT);
    std::fs::rename(&tmp, &path).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Writes `text` to `path`; with `private`, readable by the owner only.
fn write(path: &PathBuf, text: &str, private: bool) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    if private {
        options.mode(0o600);
    }
    options
        .open(path)
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "watcher-ca-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn created_once_then_reopened() {
        let dir = temp_dir();
        let ca = Ca::open_or_create(&dir).unwrap();
        let again = Ca::open_or_create(&dir).unwrap();
        assert_eq!(ca.fingerprint(), again.fingerprint());
        let mode = std::os::unix::fs::PermissionsExt::mode(
            &std::fs::metadata(dir.join("ca/ca.key"))
                .unwrap()
                .permissions(),
        );
        assert_eq!(mode & 0o777, 0o600);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn signs_node_requests_with_its_own_fields() {
        let dir = temp_dir();
        let ca = Ca::open_or_create(&dir).unwrap();
        let key = KeyPair::generate().unwrap();
        // The node asks for a server certificate for the hub's name; it gets
        // a client certificate for its own name.
        let mut asked = CertificateParams::new(vec![HUB_NAME.to_string()]).unwrap();
        asked.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let csr = asked.serialize_request(&key).unwrap();
        let cert = ca.sign_node("site-a", csr.der()).unwrap();
        let der = CertificateDer::from_pem_slice(cert.pem.as_bytes()).unwrap();
        assert_eq!(cert.fingerprint, fingerprint(&der));
        let (_, parsed) = x509_parser::parse_x509_certificate(&der).unwrap();
        assert_eq!(
            parsed
                .subject()
                .iter_common_name()
                .next()
                .unwrap()
                .as_str()
                .unwrap(),
            "site-a"
        );
        let eku = parsed.extended_key_usage().unwrap().unwrap().value;
        assert!(eku.client_auth && !eku.server_auth);
        assert!(parsed.subject_alternative_name().unwrap().is_none());

        assert!(ca.sign_node("site-a", b"not a request").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
