use serde::Deserialize;
use std::path::{Path, PathBuf};

/// The bootstrap file, `watcher.toml`: what is needed before the object
/// configuration and the database can be read.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bootstrap {
    #[serde(default)]
    pub role: Role,
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
    pub database: Option<Database>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Hub and local node in one process.
    #[default]
    Standalone,
    Hub,
    Node,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Database {
    /// PostgreSQL connection URL, e.g. `postgres://watcher@localhost/watcher`.
    pub url: String,
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
        bootstrap.conf_dir = base.join(&bootstrap.conf_dir);
        bootstrap.data_dir = base.join(&bootstrap.data_dir);
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
        assert_eq!(b.role, Role::Hub);
        assert_eq!(b.conf_dir, dir.join("conf.d"));
        assert_eq!(b.listen, "127.0.0.1:8080");
        assert_eq!(b.database.unwrap().url, "postgres://localhost/w");

        std::fs::write(&path, "rol = \"hub\"\n").unwrap();
        let err = Bootstrap::load(&path).unwrap_err().to_string();
        assert!(err.contains("unknown field"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
