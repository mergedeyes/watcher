//! HTTPS for the web UI and API: configured certificate files, or a
//! self-signed certificate generated on first start.

use anyhow::{Context, Result};
use axum_server::tls_rustls::RustlsConfig;
use std::path::Path;
use tracing::{info, warn};
use watcher_config::Tls;

/// The server config for `tls`. A missing self-signed certificate is
/// created for `names` (host names and addresses it should be valid for).
pub async fn config(tls: &Tls, data_dir: &Path, names: &[String]) -> Result<RustlsConfig> {
    // Fails harmlessly if a provider is already installed.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (cert, key) = tls
        .files(data_dir)
        .context("[tls]: set cert and key, or self_signed = true")?;
    if tls.self_signed && !(cert.exists() && key.exists()) {
        generate(&cert, &key, names)?;
        info!("created a self-signed certificate for {}", names.join(", "));
    }
    if tls.self_signed {
        warn!("serving HTTPS with a self-signed certificate; browsers will warn");
    }
    RustlsConfig::from_pem_file(&cert, &key)
        .await
        .with_context(|| format!("cannot use {} / {}", cert.display(), key.display()))
}

fn generate(cert: &Path, key: &Path, names: &[String]) -> Result<()> {
    let generated = rcgen::generate_simple_self_signed(names.to_vec())
        .context("cannot create a self-signed certificate")?;
    if let Some(dir) = cert.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::init_cmd::write_file(key, &generated.signing_key.serialize_pem(), true)
        .with_context(|| format!("cannot write {}", key.display()))?;
    crate::init_cmd::write_file(cert, &generated.cert.pem(), false)
        .with_context(|| format!("cannot write {}", cert.display()))?;
    Ok(())
}

/// Names for a self-signed certificate: localhost, this machine, the
/// listen address and the host of `public_url`.
pub fn names(listen: std::net::SocketAddr, public_url: Option<&str>) -> Vec<String> {
    let mut names = vec!["localhost".to_string(), crate::init_cmd::hostname()];
    if !listen.ip().is_unspecified() {
        names.push(listen.ip().to_string());
    }
    if let Some(host) = public_url
        .and_then(|u| u.split_once("://"))
        .map(|(_, rest)| rest.split(['/', ':']).next().unwrap_or_default())
        .filter(|h| !h.is_empty())
    {
        names.push(host.to_string());
    }
    names.sort();
    names.dedup();
    names
}
