use crate::CheckDirs;
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// The bootstrap file, `watcher.toml`: what is needed before the object
/// configuration and the database can be read.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bootstrap {
    #[serde(default)]
    pub role: InstanceRole,
    /// Directory with the object files. Relative paths are resolved against
    /// the directory of `watcher.toml`.
    #[serde(default = "default_conf_dir")]
    pub conf_dir: PathBuf,
    /// Directory for runtime data (buffers, state). Resolved like `conf_dir`.
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,
    /// Address of the HTTP API and web UI.
    #[serde(default = "default_listen")]
    pub listen: String,
    /// Address users reach this instance at, e.g. `https://watcher.example.com`.
    /// Used for links in notifications.
    pub public_url: Option<String>,
    pub database: Option<Database>,
    /// Mail server for email notifications.
    pub smtp: Option<Smtp>,
    /// Who edits the object files.
    #[serde(default)]
    pub config_mode: ConfigMode,
    /// Name of this instance, shown in the web UI and notifications.
    pub instance_name: Option<String>,
    /// IANA time zone of downtime windows and time periods, e.g.
    /// `Europe/Berlin`. Default: the system's.
    pub timezone: Option<String>,
    /// Serve HTTPS instead of HTTP.
    pub tls: Option<Tls>,
    /// Where bare command names are looked up before PATH, in order.
    /// Relative paths are resolved like `conf_dir`. Default: `checks/`,
    /// then the monitoring-plugins directories that exist.
    pub check_dirs: Option<Vec<PathBuf>>,
    /// The directory of watcher.toml.
    #[serde(skip)]
    pub dir: PathBuf,
}

/// Certificate for HTTPS.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tls {
    /// PEM certificate chain. Relative paths are resolved like `conf_dir`.
    pub cert: Option<PathBuf>,
    /// PEM private key.
    pub key: Option<PathBuf>,
    /// Generate a self-signed certificate in `data_dir/tls/` instead.
    #[serde(default)]
    pub self_signed: bool,
    /// Get certificates from an ACME CA such as Let's Encrypt instead.
    pub acme: Option<Acme>,
}

/// Let's Encrypt's ACME directory, the default CA.
pub const LETS_ENCRYPT: &str = "https://acme-v02.api.letsencrypt.org/directory";

/// Certificates from an ACME CA (RFC 8555). The CA checks each name with
/// TLS-ALPN-01: it connects to port 443 of the name, which must reach this
/// server.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Acme {
    /// Names the certificate is for. Default: the host of `public_url`.
    #[serde(default)]
    pub domains: Vec<String>,
    /// Where the CA writes about expiring certificates and problems.
    pub email: Option<String>,
    /// The CA's ACME directory URL. Default: Let's Encrypt.
    #[serde(default = "lets_encrypt")]
    pub directory: String,
    /// PEM root certificate of the ACME server's own HTTPS, for internal CAs
    /// that the system doesn't trust. Relative paths are resolved like
    /// `conf_dir`.
    pub ca: Option<PathBuf>,
    /// External account binding, for CAs that require it (ZeroSSL, Google):
    /// the key ID and the HMAC key (base64url) from the CA's console.
    pub eab_kid: Option<String>,
    pub eab_hmac_key: Option<String>,
}

fn lets_encrypt() -> String {
    LETS_ENCRYPT.into()
}

impl Acme {
    /// Checks the settings and fills in the domains from `public_url`.
    pub fn resolve(&mut self, public_url: Option<&str>) -> Result<(), String> {
        if self.domains.is_empty()
            && let Some(host) = public_url.and_then(url_host)
        {
            self.domains.push(host.to_string());
        }
        if self.domains.is_empty() {
            return Err("[tls.acme]: set domains, or a public_url with a host name".into());
        }
        for name in &mut self.domains {
            *name = name.trim().to_ascii_lowercase();
            if name.parse::<std::net::IpAddr>().is_ok()
                || name.is_empty()
                || name.contains(['/', ':', ' '])
            {
                return Err(format!(
                    "[tls.acme] domains: {name:?} is not a host name (IP addresses don't work)"
                ));
            }
        }
        if !(self.directory.starts_with("https://") || self.directory.starts_with("http://")) {
            return Err(format!(
                "[tls.acme] directory: {:?} is not a URL",
                self.directory
            ));
        }
        let set = |value: &Option<String>| value.as_deref().is_some_and(|v| !v.trim().is_empty());
        if (self.eab_kid.is_some() || self.eab_hmac_key.is_some())
            && !(set(&self.eab_kid) && set(&self.eab_hmac_key))
        {
            return Err("[tls.acme]: set both eab_kid and eab_hmac_key, or neither".into());
        }
        Ok(())
    }
}

/// The host of an http(s) URL, if it is a name (not an IP address).
fn url_host(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let host = rest.split(['/', ':', '?', '#']).next()?;
    (!host.is_empty() && !host.starts_with('[') && host.parse::<std::net::IpAddr>().is_err())
        .then_some(host)
}

impl Tls {
    /// Certificate and key files: the configured ones, or the self-signed
    /// pair below `data_dir`. `None` with ACME, which keeps its own.
    pub fn files(&self, data_dir: &Path) -> Option<(PathBuf, PathBuf)> {
        if self.acme.is_some() {
            return None;
        }
        match (&self.cert, &self.key, self.self_signed) {
            (Some(cert), Some(key), false) => Some((cert.clone(), key.clone())),
            (None, None, true) => {
                let dir = data_dir.join("tls");
                Some((dir.join("cert.pem"), dir.join("key.pem")))
            }
            _ => None,
        }
    }
}

/// Who edits the object files.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigMode {
    /// Web UI and text editors; file changes are picked up automatically.
    #[default]
    Hybrid,
    /// Text editors only; the web UI shows the config read-only.
    Files,
    /// The web UI only; file changes are not watched (SIGHUP still reloads).
    Ui,
}

impl ConfigMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ConfigMode::Hybrid => "hybrid",
            ConfigMode::Files => "files",
            ConfigMode::Ui => "ui",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstanceRole {
    /// Hub and local node in one process.
    #[default]
    Standalone,
    Hub,
    Node,
}

impl InstanceRole {
    pub fn as_str(self) -> &'static str {
        match self {
            InstanceRole::Standalone => "standalone",
            InstanceRole::Hub => "hub",
            InstanceRole::Node => "node",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Database {
    /// PostgreSQL connection URL, e.g. `postgres://watcher@localhost/watcher`.
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Smtp {
    pub host: String,
    /// Defaults to 587 for STARTTLS, 465 for TLS and 25 without encryption.
    pub port: Option<u16>,
    #[serde(default)]
    pub tls: SmtpTls,
    pub username: Option<String>,
    /// The password, or use `password_file` to keep it out of this file.
    pub password: Option<String>,
    /// File holding the password. Relative paths are resolved like `conf_dir`.
    pub password_file: Option<PathBuf>,
    /// Sender, e.g. `Watcher <watcher@example.com>`.
    pub from: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SmtpTls {
    /// Plain connection upgraded with STARTTLS (required).
    #[default]
    Starttls,
    /// TLS from the start (SMTPS).
    Tls,
    /// No encryption, e.g. a relay on localhost.
    None,
}

impl Smtp {
    pub fn port(&self) -> u16 {
        self.port.unwrap_or(match self.tls {
            SmtpTls::Starttls => 587,
            SmtpTls::Tls => 465,
            SmtpTls::None => 25,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("{path}: {message}")]
    Invalid { path: PathBuf, message: String },
}

fn default_conf_dir() -> PathBuf {
    "conf.d".into()
}

fn default_data_dir() -> PathBuf {
    "data".into()
}

fn default_listen() -> String {
    "127.0.0.1:8080".into()
}

impl Bootstrap {
    pub fn check_dirs(&self) -> CheckDirs {
        match &self.check_dirs {
            Some(dirs) => CheckDirs::new(dirs.clone()),
            None => CheckDirs::default_for(&self.dir),
        }
    }

    pub fn load(path: &Path) -> Result<Self, BootstrapError> {
        let text = std::fs::read_to_string(path).map_err(|source| BootstrapError::Read {
            path: path.into(),
            source,
        })?;
        let mut bootstrap: Bootstrap =
            toml::from_str(&text).map_err(|source| BootstrapError::Parse {
                path: path.into(),
                source,
            })?;
        let base = path.parent().unwrap_or(Path::new("."));
        bootstrap.dir = base.to_path_buf();
        bootstrap.conf_dir = base.join(&bootstrap.conf_dir);
        for dir in bootstrap.check_dirs.iter_mut().flatten() {
            *dir = base.join(&*dir);
        }
        bootstrap.data_dir = base.join(&bootstrap.data_dir);
        if let Some(file) = bootstrap
            .smtp
            .as_mut()
            .and_then(|s| s.password_file.as_mut())
        {
            *file = base.join(&*file);
        }
        if let Some(name) = &bootstrap.timezone
            && let Err(err) = jiff::tz::TimeZone::get(name)
        {
            return Err(BootstrapError::Invalid {
                path: path.into(),
                message: format!("timezone {name:?}: {err}"),
            });
        }
        if let Some(tls) = &mut bootstrap.tls {
            let invalid = |message: String| BootstrapError::Invalid {
                path: path.into(),
                message,
            };
            let acme_only = tls.cert.is_none() && tls.key.is_none() && !tls.self_signed;
            let has_files = tls.files(&bootstrap.data_dir).is_some();
            match &mut tls.acme {
                Some(_) if !acme_only => {
                    return Err(invalid(
                        "[tls]: [tls.acme] replaces cert, key and self_signed; remove them".into(),
                    ));
                }
                Some(acme) => {
                    acme.resolve(bootstrap.public_url.as_deref())
                        .map_err(invalid)?;
                    if let Some(ca) = &mut acme.ca {
                        *ca = base.join(&*ca);
                    }
                }
                None if !has_files => {
                    return Err(invalid(
                        "[tls]: set cert and key, self_signed = true, or [tls.acme]".into(),
                    ));
                }
                None => {}
            }
            for file in [&mut tls.cert, &mut tls.key].into_iter().flatten() {
                *file = base.join(&*file);
            }
        }
        Ok(bootstrap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_relative_paths() {
        let dir = std::env::temp_dir().join(format!("watcher-bootstrap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("watcher.toml");
        std::fs::write(
            &path,
            "role = \"hub\"\n[database]\nurl = \"postgres://localhost/w\"\n",
        )
        .unwrap();
        let b = Bootstrap::load(&path).unwrap();
        assert_eq!(b.role, InstanceRole::Hub);
        assert_eq!(b.conf_dir, dir.join("conf.d"));
        assert_eq!(b.listen, "127.0.0.1:8080");
        assert_eq!(b.check_dirs().dirs()[0], dir.join("checks"));
        assert_eq!(b.database.unwrap().url, "postgres://localhost/w");

        std::fs::write(&path, "check_dirs = [\"plugins\", \"/opt/checks\"]\n").unwrap();
        let b = Bootstrap::load(&path).unwrap();
        assert_eq!(
            b.check_dirs().dirs(),
            [dir.join("plugins"), PathBuf::from("/opt/checks")]
        );

        std::fs::write(
            &path,
            "public_url = \"https://w.example\"\n[smtp]\nhost = \"mail\"\nfrom = \"w@example.com\"\npassword_file = \"smtp.pass\"\n",
        )
        .unwrap();
        let b = Bootstrap::load(&path).unwrap();
        let smtp = b.smtp.unwrap();
        assert_eq!((smtp.tls, smtp.port()), (SmtpTls::Starttls, 587));
        assert_eq!(smtp.password_file.unwrap(), dir.join("smtp.pass"));
        assert_eq!(b.public_url.as_deref(), Some("https://w.example"));

        std::fs::write(&path, "[tls]\ncert = \"c.pem\"\nkey = \"k.pem\"\n").unwrap();
        let tls = Bootstrap::load(&path).unwrap().tls.unwrap();
        assert_eq!(
            tls.files(Path::new("/d")),
            Some((dir.join("c.pem"), dir.join("k.pem")))
        );
        std::fs::write(&path, "[tls]\nself_signed = true\n").unwrap();
        let b = Bootstrap::load(&path).unwrap();
        assert_eq!(
            b.tls.unwrap().files(&b.data_dir).unwrap().0,
            dir.join("data/tls/cert.pem")
        );
        std::fs::write(&path, "[tls]\ncert = \"c.pem\"\n").unwrap();
        assert!(Bootstrap::load(&path).is_err());

        // ACME: the domain from public_url, the default CA.
        std::fs::write(
            &path,
            "public_url = \"https://Watcher.Example.com:8443/\"\n[tls.acme]\nca = \"root.pem\"\n",
        )
        .unwrap();
        let b = Bootstrap::load(&path).unwrap();
        let tls = b.tls.unwrap();
        let acme = tls.acme.as_ref().unwrap();
        assert_eq!(acme.domains, ["watcher.example.com"]);
        assert_eq!(acme.directory, LETS_ENCRYPT);
        assert_eq!(acme.ca.as_deref(), Some(dir.join("root.pem").as_path()));
        assert_eq!(tls.files(&b.data_dir), None);
        let err = |text: &str| {
            std::fs::write(&path, text).unwrap();
            Bootstrap::load(&path).unwrap_err().to_string()
        };
        assert!(err("[tls.acme]\n").contains("set domains"));
        assert!(err("public_url = \"https://10.0.0.1/\"\n[tls.acme]\n").contains("set domains"));
        assert!(err("[tls.acme]\ndomains = [\"10.0.0.1\"]\n").contains("not a host name"));
        assert!(
            err("[tls]\nself_signed = true\n[tls.acme]\ndomains = [\"w.example\"]\n")
                .contains("replaces")
        );
        assert!(
            err("[tls.acme]\ndomains = [\"w.example\"]\neab_kid = \"k\"\n")
                .contains("eab_hmac_key")
        );
        assert!(
            err("[tls.acme]\ndomains = [\"w.example\"]\neab_kid = \"k\"\neab_hmac_key = \"\"\n")
                .contains("eab_hmac_key")
        );

        std::fs::write(&path, "rol = \"hub\"\n").unwrap();
        let err = Bootstrap::load(&path).unwrap_err().to_string();
        assert!(err.contains("unknown field"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
