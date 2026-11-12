//! The protocol between nodes and their hub: gRPC over mutual TLS, defined
//! in `proto/link.proto`, plus the join token both sides read.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use std::fmt;
use std::str::FromStr;

/// Generated from `proto/link.proto`.
#[allow(clippy::all)]
pub mod link {
    tonic::include_proto!("watcher.link.v1");
}

pub use link::*;

/// The name in the hub's certificate. Nodes check the certificate against
/// this name, whatever address they dial, since the hub's own CA issued it.
pub const HUB_NAME: &str = "hub.watcher.internal";

/// Port nodes connect to when the hub URL doesn't name one.
pub const DEFAULT_PORT: u16 = 7443;

/// Prefix of join tokens, so they are recognisable.
const TOKEN_PREFIX: &str = "wj_";

/// Host and port of a hub's node link: `https://hub.example.com:7443`, or
/// without scheme or port (`hub.example.com` means port 7443). IPv6
/// addresses go in brackets.
pub fn hub_address(url: &str) -> Result<(String, u16), String> {
    let bad = || format!("hub address {url:?}: expected https://host:port");
    let rest = url.trim();
    let rest = match rest.split_once("://") {
        Some(("https", rest)) => rest,
        Some(_) => return Err(bad()),
        None => rest,
    };
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let (host, port) = match rest.strip_prefix('[') {
        Some(v6) => {
            let (host, after) = v6.split_once(']').ok_or_else(bad)?;
            match after {
                "" => (host, None),
                _ => (host, Some(after.strip_prefix(':').ok_or_else(bad)?)),
            }
        }
        None => match rest.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (rest, None),
        },
    };
    if host.is_empty() || host.contains(['/', '@', ' ', '[', ']']) {
        return Err(bad());
    }
    let port = match port {
        Some(port) => port.parse().map_err(|_| bad())?,
        None => DEFAULT_PORT,
    };
    Ok((host.to_string(), port))
}

/// `https://host:port` of a hub address, as [`hub_address`] reads it.
pub fn hub_url(host: &str, port: u16) -> String {
    match host.contains(':') {
        true => format!("https://[{host}]:{port}"),
        false => format!("https://{host}:{port}"),
    }
}

/// SHA-256 of a DER certificate, base64url: how the hub knows a node's
/// certificate, and how a join token names the hub's CA.
pub fn fingerprint(der: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(der))
}

/// What a node needs to join a hub once: a one-time secret, and the
/// fingerprint of the hub's CA certificate, so the node can trust the hub
/// on its first connection. Written `wj_<secret>.<fingerprint>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinToken {
    /// `wj_` and 22 characters; the hub keeps only its hash.
    pub secret: String,
    /// [`fingerprint`] of the hub's CA certificate.
    pub ca: String,
}

impl JoinToken {
    /// A new token with a random secret, for the CA with fingerprint `ca`.
    pub fn generate(ca: &str) -> JoinToken {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).expect("the OS random number generator works");
        JoinToken {
            secret: format!("{TOKEN_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes)),
            ca: ca.to_string(),
        }
    }

    /// The SHA-256 of `secret`, which the hub stores instead of it.
    pub fn hash(secret: &str) -> Vec<u8> {
        Sha256::digest(secret.as_bytes()).to_vec()
    }
}

impl fmt::Display for JoinToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.secret, self.ca)
    }
}

impl FromStr for JoinToken {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let bad = || "not a join token (it looks like wj_….…; get one on the hub)".to_string();
        let (secret, ca) = s.trim().split_once('.').ok_or_else(bad)?;
        let valid = |part: &str, len: usize| {
            URL_SAFE_NO_PAD
                .decode(part)
                .is_ok_and(|bytes| bytes.len() == len)
        };
        let body = secret.strip_prefix(TOKEN_PREFIX).ok_or_else(bad)?;
        if !valid(body, 16) || !valid(ca, 32) {
            return Err(bad());
        }
        Ok(JoinToken {
            secret: secret.to_string(),
            ca: ca.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_addresses() {
        let ok = |url: &str, host: &str, port: u16| {
            assert_eq!(hub_address(url), Ok((host.to_string(), port)), "{url}");
            let (h, p) = hub_address(url).unwrap();
            assert_eq!(hub_address(&hub_url(&h, p)), Ok((h, p)));
        };
        ok("https://hub.example.com:8443", "hub.example.com", 8443);
        ok("https://hub.example.com/", "hub.example.com", 7443);
        ok("hub.example.com", "hub.example.com", 7443);
        ok("192.0.2.1:9000", "192.0.2.1", 9000);
        ok("https://[2001:db8::1]:9000", "2001:db8::1", 9000);
        ok("[2001:db8::1]", "2001:db8::1", 7443);
        for bad in [
            "",
            "http://hub",
            "https://",
            "hub:port",
            "hub:70000",
            "https://hub/path",
            "user@hub",
            "2001:db8::1",
            "[2001:db8::1",
        ] {
            assert!(hub_address(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn tokens_round_trip() {
        let ca = fingerprint(b"certificate");
        let token = JoinToken::generate(&ca);
        let text = token.to_string();
        assert!(text.starts_with("wj_"));
        assert_eq!(text.parse::<JoinToken>(), Ok(token.clone()));
        // Pasted with a line break.
        assert_eq!(format!(" {text}\n").parse::<JoinToken>(), Ok(token));
    }

    #[test]
    fn broken_tokens_are_refused() {
        let token = JoinToken::generate(&fingerprint(b"certificate")).to_string();
        for bad in [
            "",
            "wj_abc",
            &token[..token.len() - 1],
            &token.replacen("wj_", "wt_", 1),
            &token.replace('.', ""),
        ] {
            assert!(bad.parse::<JoinToken>().is_err(), "{bad:?}");
        }
    }
}
