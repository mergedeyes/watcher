use crate::user_cmd::DbArgs;
use anyhow::{Context, Result, bail};
use std::io::BufRead;
use std::path::PathBuf;
use std::process::ExitCode;
use watcher_exec::ssh::{self, Credential, Sealer};
use watcher_hub::ssh::check_name;
use watcher_store::Store;

#[derive(clap::Subcommand)]
pub enum SecretCommand {
    /// Generate an ed25519 key and print the public key for authorized_keys.
    New {
        name: String,
        /// Replace a secret of the same name.
        #[arg(long)]
        replace: bool,
    },
    /// Store an existing ed25519 or ECDSA private key (OpenSSH, PEM or PuTTY
    /// format; not RSA).
    Import {
        name: String,
        file: PathBuf,
        /// Read the key's passphrase from the first line of stdin; otherwise
        /// it is asked for if the key is encrypted.
        #[arg(long)]
        passphrase_stdin: bool,
        #[arg(long)]
        replace: bool,
    },
    /// Store a password.
    Password {
        name: String,
        /// Read the password from the first line of stdin instead of asking.
        #[arg(long)]
        password_stdin: bool,
        #[arg(long)]
        replace: bool,
    },
    /// Print a key's public key.
    Show { name: String },
    /// List secrets (names, kinds, fingerprints; never the secrets).
    List,
    /// Delete a secret.
    Remove { name: String },
}

#[derive(clap::Subcommand)]
pub enum HostKeyCommand {
    /// Pinned host keys, and changed keys waiting for acceptance.
    List,
    /// Pin the changed key a host presented: ADDRESS or ADDRESS:PORT.
    Accept { host: String },
    /// Forget a host's key; the next one it presents is pinned.
    Forget { host: String },
}

pub async fn secret(command: SecretCommand, db: DbArgs) -> Result<ExitCode> {
    let store = db.store().await?;
    let put = |name: String, credential: Credential, replace: bool| {
        let (store, db) = (&store, &db);
        async move {
            let name = name.trim().to_string();
            check_name(&name).map_err(anyhow::Error::msg)?;
            let sealer = Sealer::open_or_create(&db.data_dir()?).map_err(anyhow::Error::msg)?;
            let sealed = sealer
                .seal(&name, &credential)
                .map_err(anyhow::Error::msg)?;
            let public_key = credential.public_key();
            let kind = credential.kind().as_str();
            store
                .put_secret(&name, kind, public_key.as_deref(), &sealed, replace)
                .await?;
            Ok::<_, anyhow::Error>((name, public_key))
        }
    };
    match command {
        SecretCommand::New { name, replace } => {
            let credential = Credential::generate(&format!("watcher-{}", name.trim()))
                .map_err(anyhow::Error::msg)?;
            let (name, public_key) = put(name, credential, replace).await?;
            // The public key alone on stdout, for `>> authorized_keys`.
            eprintln!("Key {name:?}; add the public key to ~/.ssh/authorized_keys on the hosts:");
            println!("{}", public_key.unwrap_or_default());
        }
        SecretCommand::Import {
            name,
            file,
            passphrase_stdin,
            replace,
        } => {
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("cannot read {}", file.display()))?;
            let credential = match passphrase_stdin {
                true => Credential::import(&text, Some(&read_line()?)),
                false => match Credential::import(&text, None) {
                    Err(msg) if msg.contains("encrypted") => {
                        let passphrase = rpassword::prompt_password("Passphrase: ")?;
                        Credential::import(&text, Some(&passphrase))
                    }
                    other => other,
                },
            }
            .map_err(anyhow::Error::msg)?;
            let fingerprint = credential.fingerprint().unwrap_or_default();
            let (name, _) = put(name, credential, replace).await?;
            println!("stored key {name:?} ({fingerprint})");
        }
        SecretCommand::Password {
            name,
            password_stdin,
            replace,
        } => {
            let password = match password_stdin {
                true => read_line()?,
                false => {
                    let first = rpassword::prompt_password("Password: ")?;
                    let second = rpassword::prompt_password("Repeat password: ")?;
                    if first != second {
                        bail!("the passwords don't match");
                    }
                    first
                }
            };
            if password.is_empty() {
                bail!("the password is empty");
            }
            let (name, _) = put(name, Credential::Password(password), replace).await?;
            println!("stored password {name:?}");
        }
        SecretCommand::Show { name } => {
            let list = store.secrets().await?;
            let Some(secret) = list.into_iter().find(|s| s.name == name) else {
                bail!("no secret {name:?}");
            };
            match secret.public_key {
                Some(key) => println!("{key}"),
                None => bail!("{name:?} is a password, not a key"),
            }
        }
        SecretCommand::List => {
            let list = store.secrets().await?;
            if list.is_empty() {
                println!("no secrets yet; create a key with: watcher secret new <name>");
            }
            for s in list {
                let fingerprint = s.public_key.as_deref().and_then(ssh::fingerprint);
                println!(
                    "{:<24} {:<9} {:<52} updated {}",
                    s.name,
                    s.kind,
                    fingerprint.unwrap_or_default(),
                    humantime::format_rfc3339_seconds(s.updated)
                );
            }
        }
        SecretCommand::Remove { name } => {
            if !store.delete_secret(&name).await? {
                bail!("no secret {name:?}");
            }
            println!("removed secret {name:?}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

pub async fn host_key(command: HostKeyCommand, db: DbArgs) -> Result<ExitCode> {
    let store: Store = db.store().await?;
    match command {
        HostKeyCommand::List => {
            let keys = store.host_keys().await?;
            if keys.is_empty() {
                println!("no host keys yet: they are pinned on the first connection");
            }
            for k in keys {
                println!(
                    "{:<32} {}  pinned {}",
                    host_port(&k.address, k.port),
                    ssh::fingerprint(&k.key).unwrap_or_default(),
                    humantime::format_rfc3339_seconds(k.pinned)
                );
                if let (Some(offered), Some(at)) = (&k.offered, k.offered_at) {
                    println!(
                        "{:<32} {}  CHANGED {}",
                        "",
                        ssh::fingerprint(offered).unwrap_or_default(),
                        humantime::format_rfc3339_seconds(at)
                    );
                }
            }
        }
        HostKeyCommand::Accept { host } => {
            let (address, port) = parse_host(&host)?;
            match store.accept_host_key(&address, port).await? {
                Some(k) => println!(
                    "pinned {} for {host}",
                    ssh::fingerprint(&k.key).unwrap_or_default()
                ),
                None => bail!("{host} presented no new key"),
            }
        }
        HostKeyCommand::Forget { host } => {
            let (address, port) = parse_host(&host)?;
            if !store.forget_host_key(&address, port).await? {
                bail!("no host key for {host}");
            }
            println!("forgot the host key of {host}; the next one is pinned");
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// `db1`, `db1:2222`, `[::1]:2222`.
fn parse_host(text: &str) -> Result<(String, u16)> {
    let port = |p: &str| {
        p.parse::<u16>()
            .with_context(|| format!("invalid port {p:?}"))
    };
    if let Some(rest) = text.strip_prefix('[') {
        let (address, after) = rest.split_once(']').context("missing ]")?;
        return match after.strip_prefix(':') {
            Some(p) => Ok((address.to_string(), port(p)?)),
            None => Ok((address.to_string(), 22)),
        };
    }
    match text.split_once(':') {
        // An IPv6 address without brackets.
        Some((_, rest)) if rest.contains(':') => Ok((text.to_string(), 22)),
        Some((address, p)) => Ok((address.to_string(), port(p)?)),
        None => Ok((text.to_string(), 22)),
    }
}

fn host_port(address: &str, port: u16) -> String {
    match (port, address.contains(':')) {
        (22, _) => address.to_string(),
        (_, true) => format!("[{address}]:{port}"),
        (_, false) => format!("{address}:{port}"),
    }
}

fn read_line() -> Result<String> {
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts() {
        let p = |t: &str| parse_host(t).unwrap();
        assert_eq!(p("db1"), ("db1".into(), 22));
        assert_eq!(p("db1:2222"), ("db1".into(), 2222));
        assert_eq!(p("[::1]:2222"), ("::1".into(), 2222));
        assert_eq!(p("fe80::1"), ("fe80::1".into(), 22));
        assert!(parse_host("db1:x").is_err());
        assert_eq!(host_port("::1", 2222), "[::1]:2222");
    }
}
