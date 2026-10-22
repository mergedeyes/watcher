use crate::config_cmd;
use crate::sinks::Sinks;
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};
use watcher_config::{Config, ConfigMode, Severity, files, load_dir};
use watcher_core::StateType;
use watcher_exec::scheduler::{Control, Event, schedule};
use watcher_hub::{AuthSettings, ConfigFiles, Hub, jobs, specs};
use watcher_notify::{Notifier, setup_warnings};
use watcher_store::{Record, Store, StoredState, ingest};

#[derive(clap::Args)]
pub struct Args {
    /// watcher.toml, or a directory of object files. Defaults to
    /// ./watcher.toml, then /etc/watcher/watcher.toml.
    path: Option<PathBuf>,

    /// Log every check result, not only state changes.
    #[arg(long)]
    all: bool,

    /// Maximum number of checks running at the same time.
    #[arg(long, default_value_t = 64)]
    max_concurrent: usize,

    /// PostgreSQL URL to store results in. Overrides [database] in watcher.toml.
    #[arg(long, env = "WATCHER_DATABASE_URL", value_name = "URL")]
    database: Option<String>,

    /// Don't store results, even if a database is configured.
    #[arg(long, conflicts_with = "database")]
    no_database: bool,

    /// Don't require a login for the web UI and API. Only allowed when
    /// listening on a loopback address.
    #[arg(long)]
    no_auth: bool,

    /// Address for the HTTP API. Overrides `listen` in watcher.toml
    /// (default 127.0.0.1:8080).
    #[arg(long, value_name = "ADDR")]
    listen: Option<SocketAddr>,
}

pub async fn run(args: Args) -> Result<ExitCode> {
    let (bootstrap, conf_dir) = config_cmd::locate(args.path.as_deref())?;
    let Some(config) = load(&conf_dir) else {
        bail!("configuration has errors");
    };
    let config = Arc::new(config);
    let mut snapshot = snapshot(&conf_dir);
    let mode = bootstrap
        .as_ref()
        .map_or(ConfigMode::default(), |b| b.config_mode);
    let data_dir = bootstrap.as_ref().map(|b| b.data_dir.clone());
    let sinks = Sinks::start(data_dir.as_deref(), &config).await?;

    let listen = match (args.listen, &bootstrap) {
        (Some(addr), _) => addr,
        (None, Some(b)) => b
            .listen
            .parse()
            .with_context(|| format!("invalid listen address {:?}", b.listen))?,
        (None, None) => SocketAddr::from(([127, 0, 0, 1], 8080)),
    };

    let (smtp, public_url) = bootstrap
        .as_ref()
        .map(|b| (b.smtp.clone(), b.public_url.clone()))
        .unwrap_or_default();
    let https = public_url
        .as_deref()
        .is_some_and(|u| u.starts_with("https://"));
    let notifier =
        Arc::new(Notifier::new(smtp.as_ref(), public_url).context("invalid [smtp] settings")?);
    for warning in setup_warnings(&config, &notifier) {
        warn!("{warning}");
    }
    let (config_tx, config_rx) = watch::channel(config.clone());
    let (notify_tx, notify_rx) = mpsc::channel(1000);
    tokio::spawn(watcher_notify::run(notifier.clone(), config_rx, notify_rx));

    let database = match args.no_database {
        true => None,
        false => args
            .database
            .or_else(|| bootstrap.and_then(|b| b.database).map(|d| d.url)),
    };
    let (storage, restored) = match database {
        Some(url) => {
            let (storage, states) = Storage::start(&url, &config).await?;
            (Some(storage), states)
        }
        None => {
            info!("no database configured; results are not stored");
            (None, HashMap::new())
        }
    };
    let initial = restored
        .iter()
        .map(|(id, s)| (id.clone(), s.state.clone()))
        .collect();

    let (control, control_rx) = mpsc::channel(64);
    let (events_tx, mut events) = mpsc::channel(256);
    let scheduler = tokio::spawn(schedule(
        jobs(&config),
        initial,
        args.max_concurrent,
        control_rx,
        events_tx,
    ));

    let store = storage.as_ref().map(|s| s.store.clone());
    let auth = auth_settings(args.no_auth, listen, https, store.as_deref()).await?;
    let hub = Hub::new(config.clone(), restored, control.clone(), store, auth);
    let (reload_tx, mut reloads) = mpsc::channel(4);
    hub.enable_config_editing(ConfigFiles {
        dir: conf_dir.clone(),
        mode,
        reload: reload_tx,
    });
    let (changed_tx, mut changed) = mpsc::channel(1);
    // Kept alive until the end of `run`.
    let _watcher = match mode {
        ConfigMode::Ui => None,
        _ => match crate::watch::watch(&conf_dir, changed_tx) {
            Ok(w) => Some(w),
            Err(err) => {
                warn!(
                    "cannot watch {} for changes ({err}); reload with SIGHUP",
                    conf_dir.display()
                );
                None
            }
        },
    };
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("cannot listen on {listen}"))?;
    let server = tokio::spawn(axum::serve(listener, hub.clone().router()).into_future());
    info!(
        "started with {} services from {}; web UI on http://{listen}",
        config.services.len(),
        conf_dir.display()
    );

    let mut running = Running {
        storage,
        sinks,
        control: &control,
        notifier: &notifier,
        notify_tx: &notify_tx,
        config_tx: &config_tx,
        hub: &hub,
    };
    let mut hangup = signal(SignalKind::hangup())?;
    let mut terminate = signal(SignalKind::terminate())?;
    loop {
        tokio::select! {
            Some(event) = events.recv() => running.record(event, args.all),
            _ = hangup.recv() => {
                snapshot = self::snapshot(&conf_dir);
                let _ = running.reload(&conf_dir, "SIGHUP").await;
            }
            Some(()) = changed.recv() => {
                // Skip events that didn't change anything, like our own
                // writes after an API change was already reloaded.
                let now = self::snapshot(&conf_dir);
                if now == snapshot {
                    continue;
                }
                snapshot = now;
                let _ = running.reload(&conf_dir, "file change").await;
            }
            Some(request) = reloads.recv() => {
                snapshot = self::snapshot(&conf_dir);
                let result = running.reload(&conf_dir, &request.by).await;
                let _ = request.reply.send(result);
            }
            _ = tokio::signal::ctrl_c() => break,
            _ = terminate.recv() => break,
        }
    }
    info!("stopping");
    server.abort();
    scheduler.abort();
    if let Some(storage) = running.storage {
        storage.shutdown().await;
    }
    running.sinks.shutdown().await;
    Ok(ExitCode::SUCCESS)
}

/// Where results go, and what a reload updates.
struct Running<'a> {
    storage: Option<Storage>,
    sinks: Sinks,
    control: &'a mpsc::Sender<Control>,
    notifier: &'a Notifier,
    notify_tx: &'a mpsc::Sender<Event>,
    config_tx: &'a watch::Sender<Arc<Config>>,
    hub: &'a Hub,
}

impl Running<'_> {
    fn record(&mut self, event: Event, all: bool) {
        print_event(&event, all);
        self.hub.record(&event);
        if event.change.is_some_and(|c| c.hard) || event.flap.is_some() {
            if self.hub.host_down(&event.id) {
                info!("no notification for {}: host is DOWN", event.id);
            } else if self.notify_tx.try_send(event.clone()).is_err() {
                warn!(
                    "notification queue is full; dropped the notification for {}",
                    event.id
                );
            }
        }
        let record = Record {
            service: event.id,
            result: event.result,
            state: event.state,
            change: event.change,
            duration: event.duration,
        };
        self.sinks.record(&record);
        if let Some(storage) = &mut self.storage {
            storage.record(record);
        }
    }

    /// Loads the object files and switches to them. On errors the running
    /// configuration stays.
    async fn reload(&mut self, conf_dir: &Path, by: &str) -> Result<(), String> {
        info!("reloading ({by})");
        let Some(config) = load(conf_dir) else {
            warn!("reload failed; keeping the running configuration");
            return Err("the configuration has errors".into());
        };
        // Register new services before their first results arrive.
        if let Some(storage) = &self.storage
            && let Err(err) = storage.sync(&config).await
        {
            error!("reload failed, cannot update the database: {err:#}");
            return Err(format!("cannot update the database: {err:#}"));
        }
        info!("reloaded: {} services", config.services.len());
        if self
            .control
            .send(Control::Reload(jobs(&config)))
            .await
            .is_err()
        {
            return Err("the scheduler is not running".into());
        }
        for warning in setup_warnings(&config, self.notifier) {
            warn!("{warning}");
        }
        let config = Arc::new(config);
        if let Err(err) = self.sinks.reload(&config).await {
            error!("{err:#}");
        }
        self.config_tx.send_replace(config.clone());
        self.hub.set_config(config);
        Ok(())
    }
}

/// Hashes of the object files, to tell whether anything changed.
fn snapshot(conf_dir: &Path) -> Vec<(String, String)> {
    files::list(conf_dir)
        .unwrap_or_default()
        .into_iter()
        .map(|f| (f.path, f.hash))
        .collect()
}

/// Login is required whenever there is a database (users live there),
/// unless `--no-auth` is given. Without authentication the server only
/// listens on loopback addresses.
async fn auth_settings(
    no_auth: bool,
    listen: SocketAddr,
    https: bool,
    store: Option<&Store>,
) -> Result<AuthSettings> {
    let loopback = listen.ip().is_loopback();
    let Some(store) = store.filter(|_| !no_auth) else {
        if !loopback {
            bail!(
                "refusing to serve on {listen} without authentication: use a loopback address, \
                 or configure a database (user accounts live there) and drop --no-auth"
            );
        }
        if no_auth {
            warn!("authentication is off (--no-auth)");
        }
        return Ok(AuthSettings::OFF);
    };
    if store.user_count().await? == 0 {
        warn!("no user accounts yet; create one with: watcher user add <name> --admin");
    }
    if !loopback && !https {
        warn!(
            "serving logins over plain HTTP on {listen}; put Watcher behind a TLS reverse proxy \
             and set public_url to its https:// address"
        );
    }
    Ok(AuthSettings {
        required: true,
        secure_cookies: https,
    })
}

/// Results going to the database.
struct Storage {
    store: Arc<Store>,
    ids: watch::Sender<Arc<HashMap<String, i32>>>,
    records: mpsc::Sender<Record>,
    task: JoinHandle<()>,
    dropped: u64,
}

impl Storage {
    /// Connects, syncs the config and returns the stored service states.
    async fn start(url: &str, config: &Config) -> Result<(Self, HashMap<String, StoredState>)> {
        let store = Store::connect(url)
            .await
            .context("cannot use the database")?;
        info!("storing results in {}", store.backend());
        let (hosts, services) = specs(config);
        let ids = store.sync(&hosts, &services).await?;
        let states = store.load_states().await?;
        if !states.is_empty() {
            info!("restored the state of {} services", states.len());
        }

        let store = Arc::new(store);
        let (ids_tx, ids_rx) = watch::channel(Arc::new(ids));
        let (records_tx, records_rx) = mpsc::channel(10_000);
        let task = tokio::spawn(ingest(store.clone(), ids_rx, records_rx));
        let storage = Storage {
            store,
            ids: ids_tx,
            records: records_tx,
            task,
            dropped: 0,
        };
        Ok((storage, states))
    }

    async fn sync(&self, config: &Config) -> Result<()> {
        let (hosts, services) = specs(config);
        let ids = self.store.sync(&hosts, &services).await?;
        self.ids.send_replace(Arc::new(ids));
        Ok(())
    }

    /// Queues a result. Drops it rather than stalling the checks when the
    /// database falls far behind.
    fn record(&mut self, record: Record) {
        if self.records.try_send(record).is_err() {
            self.dropped += 1;
            if self.dropped.is_power_of_two() {
                warn!(
                    "database is behind; {} results dropped so far",
                    self.dropped
                );
            }
        }
    }

    /// Writes what is still queued.
    async fn shutdown(self) {
        drop(self.records);
        if tokio::time::timeout(Duration::from_secs(10), self.task)
            .await
            .is_err()
        {
            warn!("gave up writing queued results after 10s");
        }
    }
}

/// Loads the config, logging diagnostics. `None` if it has errors.
fn load(conf_dir: &Path) -> Option<Config> {
    let (config, diagnostics) = match load_dir(conf_dir) {
        Ok(loaded) => (Some(loaded.config), loaded.warnings),
        Err(diagnostics) => (None, diagnostics),
    };
    for d in &diagnostics {
        match d.severity {
            Severity::Error => error!("{d}"),
            Severity::Warning => warn!("{d}"),
        }
    }
    config
}

fn print_event(event: &Event, all: bool) {
    let s = &event.state;
    if let Some(started) = event.flap {
        let what = if started { "started" } else { "stopped" };
        info!(
            "! {:<28} flapping {what} ({:.1}% state change)",
            event.id,
            s.history.percent_change()
        );
    }
    let kind = match s.state_type {
        StateType::Hard => "HARD".to_string(),
        StateType::Soft => format!("SOFT {}", s.attempt),
    };
    let line = match event.change {
        Some(change) => {
            let marker = if change.hard { "!" } else { " " };
            let what = match change.from {
                None => format!("new -> {} ({kind})", s.state),
                // Only the state type changed: a problem got confirmed.
                Some((state, _)) if state == s.state => format!("{state} SOFT -> {kind}"),
                Some((state, _)) => format!("{state} -> {} ({kind})", s.state),
            };
            format!("{marker} {:<28} {what}", event.id)
        }
        None if all => format!("  {:<28} {} ({kind})", event.id, s.state),
        None => return,
    };
    info!("{line}  {}", event.result.message);
}
