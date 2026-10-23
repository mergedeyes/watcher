use crate::config_cmd;
use anyhow::{Context, Result, bail};
use std::io::BufRead;
use std::path::PathBuf;
use std::process::ExitCode;
use watcher_hub::auth::{hash_password, new_api_token};
use watcher_store::{Store, TokenRef};

/// Shortest password accepted.
use watcher_hub::auth::MIN_PASSWORD;

/// Where the user accounts are: the database from watcher.toml or --database.
#[derive(clap::Args)]
pub struct DbArgs {
    /// watcher.toml with the [database] section. Defaults to ./watcher.toml,
    /// then /etc/watcher/watcher.toml.
    #[arg(long, short = 'c', value_name = "FILE", global = true)]
    config: Option<PathBuf>,

    /// PostgreSQL URL; overrides [database] in watcher.toml.
    #[arg(long, env = "WATCHER_DATABASE_URL", value_name = "URL", global = true)]
    database: Option<String>,
}

impl DbArgs {
    /// `data_dir` from watcher.toml (where `secret.key` is).
    pub fn data_dir(&self) -> Result<PathBuf> {
        let (bootstrap, _) = config_cmd::locate(self.config.as_deref())?;
        match bootstrap {
            Some(b) => Ok(b.data_dir),
            None => bail!("secrets need watcher.toml: its data_dir holds secret.key"),
        }
    }

    pub async fn store(&self) -> Result<Store> {
        let url = match &self.database {
            Some(url) => url.clone(),
            None => {
                let (bootstrap, _) = config_cmd::locate(self.config.as_deref())?;
                match bootstrap.and_then(|b| b.database) {
                    Some(db) => db.url,
                    None => bail!(
                        "no database configured: user accounts need [database] in watcher.toml or --database"
                    ),
                }
            }
        };
        Store::connect(&url)
            .await
            .context("cannot use the database")
    }
}

#[derive(clap::Subcommand)]
pub enum UserCommand {
    /// Create a user account.
    Add {
        name: String,
        /// May manage other users (once user management exists in the UI).
        #[arg(long)]
        admin: bool,
        /// Read the password from the first line of stdin instead of asking.
        #[arg(long)]
        password_stdin: bool,
    },
    /// Set a new password; ends all of the user's sessions.
    Passwd {
        name: String,
        #[arg(long)]
        password_stdin: bool,
    },
    /// List user accounts.
    List,
    /// Delete a user account with its sessions and API tokens.
    Remove { name: String },
}

#[derive(clap::Subcommand)]
pub enum TokenCommand {
    /// Create an API token for a user and print it (it is shown only once).
    Create {
        user: String,
        /// What the token is for, e.g. "backup cron job".
        name: String,
    },
    /// List a user's API tokens.
    List { user: String },
    /// Revoke a user's API token.
    Revoke { user: String, name: String },
}

pub async fn user(command: UserCommand, db: DbArgs) -> Result<ExitCode> {
    let store = db.store().await?;
    match command {
        UserCommand::Add {
            name,
            admin,
            password_stdin,
        } => {
            let name = name.trim();
            if name.is_empty() || name.len() > 64 || name.chars().any(char::is_whitespace) {
                bail!("usernames are 1 to 64 characters without spaces");
            }
            let hash =
                hash_password(&read_password(password_stdin)?).map_err(anyhow::Error::msg)?;
            let user = store.create_user(name, &hash, admin).await?;
            println!(
                "created user {}{}",
                user.username,
                if admin { " (admin)" } else { "" }
            );
        }
        UserCommand::Passwd {
            name,
            password_stdin,
        } => {
            let hash =
                hash_password(&read_password(password_stdin)?).map_err(anyhow::Error::msg)?;
            if !store.set_password(&name, &hash).await? {
                bail!("no user {name:?}");
            }
            println!("password changed; {name} has to log in again");
        }
        UserCommand::List => {
            let users = store.users().await?;
            if users.is_empty() {
                println!("no users yet; create one with: watcher user add <name> --admin");
            }
            for u in users {
                let mut flags = Vec::new();
                if u.admin {
                    flags.push("admin");
                }
                if u.disabled {
                    flags.push("disabled");
                }
                let last = u.last_login.map_or("never logged in".to_string(), |t| {
                    format!("last login {}", humantime::format_rfc3339_seconds(t))
                });
                println!("{:<24} {:<16} {last}", u.username, flags.join(","));
            }
        }
        UserCommand::Remove { name } => {
            if !store.delete_user(&name).await? {
                bail!("no user {name:?}");
            }
            println!("removed user {name}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

pub async fn token(command: TokenCommand, db: DbArgs) -> Result<ExitCode> {
    let store = db.store().await?;
    let find = |name: &str| {
        let name = name.to_string();
        let store = &store;
        async move {
            match store.login_user(&name).await? {
                Some((user, _)) => Ok(user),
                None => bail!("no user {name:?}"),
            }
        }
    };
    match command {
        TokenCommand::Create { user, name } => {
            let user = find(&user).await?;
            let (secret, hash) = new_api_token();
            store.create_token(user.id, name.trim(), &hash).await?;
            // The token alone on stdout, so scripts can capture it.
            eprintln!(
                "API token {:?} for {} (shown only now):",
                name.trim(),
                user.username
            );
            println!("{secret}");
            eprintln!("Use it as: Authorization: Bearer <token>");
        }
        TokenCommand::List { user } => {
            let user = find(&user).await?;
            for t in store.tokens(user.id).await? {
                let used = t.last_used.map_or("never used".to_string(), |u| {
                    format!("last used {}", humantime::format_rfc3339_seconds(u))
                });
                println!(
                    "{:<32} created {}  {used}",
                    t.name,
                    humantime::format_rfc3339_seconds(t.created)
                );
            }
        }
        TokenCommand::Revoke { user, name } => {
            let user = find(&user).await?;
            if !store.delete_token(user.id, TokenRef::Name(&name)).await? {
                bail!("{} has no token {name:?}", user.username);
            }
            println!("revoked token {name:?} of {}", user.username);
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Asks for a password twice, or reads one line from stdin.
fn read_password(from_stdin: bool) -> Result<String> {
    let password = if from_stdin {
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        line.trim_end_matches(['\r', '\n']).to_string()
    } else {
        let first = rpassword::prompt_password("Password: ")?;
        let second = rpassword::prompt_password("Repeat password: ")?;
        if first != second {
            bail!("the passwords don't match");
        }
        first
    };
    if password.chars().count() < MIN_PASSWORD {
        bail!("use a password of at least {MIN_PASSWORD} characters");
    }
    Ok(password)
}

#[derive(clap::Subcommand)]
pub enum DbCommand {
    /// Create a read-only login for Grafana, BI tools or SQL, or set its
    /// password. It sees hosts, services, states, events and metrics
    /// (v_metrics, v_service_state, ...), not accounts. Run it again after
    /// upgrades. Needs a database user with CREATEROLE.
    Reader {
        /// Role name, e.g. grafana.
        name: String,
        /// Read the password from the first line of stdin; otherwise one is
        /// generated and printed.
        #[arg(long)]
        password_stdin: bool,
    },
}

pub async fn database(command: DbCommand, db: DbArgs) -> Result<ExitCode> {
    let store = db.store().await?;
    match command {
        DbCommand::Reader {
            name,
            password_stdin,
        } => {
            let generated = !password_stdin;
            let password = if password_stdin {
                read_password(true)?
            } else {
                let (secret, _) = new_api_token();
                secret.trim_start_matches("wt_").to_string()
            };
            store
                .grant_reader(&name, &password)
                .await
                .with_context(|| {
                    format!("cannot create role {name:?} (the database user needs CREATEROLE)")
                })?;
            eprintln!(
                "Read-only role {name:?}: {}",
                watcher_store::READABLE.join(", ")
            );
            if generated {
                // The password alone on stdout, so scripts can capture it.
                eprintln!("Password (shown only now):");
                println!("{password}");
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
