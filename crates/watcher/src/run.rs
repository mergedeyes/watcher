use crate::config_cmd;
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
use watcher_config::{Config, Severity, load_dir};
use watcher_core::StateType;
use watcher_exec::scheduler::{Control, Event, schedule};
use watcher_hub::{Hub, jobs, specs};
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

    let listen = match (args.listen, &bootstrap) {
        (Some(addr), _) => addr,
        (None, Some(b)) => b
            .listen
            .parse()
            .with_context(|| format!("invalid listen address {:?}", b.listen))?,
        (None, None) => SocketAddr::from(([127, 0, 0, 1], 8080)),
    };
    // There are no user accounts yet, so the API must not be reachable
    // from the network.
    if !listen.ip().is_loopback() {
        bail!(
            "refusing to serve the API on {listen}: authentication is not implemented yet, \
             so only loopback addresses are allowed"
        );
    }

    let database = match args.no_database {
        true => None,
        false => args
            .database
            .or_else(|| bootstrap.and_then(|b| b.database).map(|d| d.url)),
    };
    let (mut storage, restored) = match database {
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
    let hub = Hub::new(config.clone(), restored, control.clone(), store);
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("cannot listen on {listen}"))?;
    let server = tokio::spawn(axum::serve(listener, hub.clone().router()).into_future());
    info!(
        "started with {} services from {}; API on http://{listen}/api/v1 (reload with SIGHUP)",
        config.services.len(),
        conf_dir.display()
    );

    let mut hangup = signal(SignalKind::hangup())?;
    let mut terminate = signal(SignalKind::terminate())?;
    loop {
        tokio::select! {
            Some(event) = events.recv() => {
                print_event(&event, args.all);
                hub.record(&event);
                if let Some(storage) = &mut storage {
                    storage.record(event);
                }
            }
            _ = hangup.recv() => {
                let Some(config) = load(&conf_dir) else {
                    warn!("reload failed; keeping the running configuration");
                    continue;
                };
                // Register new services before their first results arrive.
                if let Some(storage) = &storage
                    && let Err(err) = storage.sync(&config).await {
                        error!("reload failed, cannot update the database: {err:#}");
                        continue;
                    }
                info!("reloaded: {} services", config.services.len());
                control.send(Control::Reload(jobs(&config))).await?;
                hub.set_config(Arc::new(config));
            }
            _ = tokio::signal::ctrl_c() => break,
            _ = terminate.recv() => break,
        }
    }
    info!("stopping");
    server.abort();
    scheduler.abort();
    if let Some(storage) = storage {
        storage.shutdown().await;
    }
    Ok(ExitCode::SUCCESS)
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
    fn record(&mut self, event: Event) {
        let record = Record {
            service: event.id,
            result: event.result,
            state: event.state,
            change: event.change,
            duration: event.duration,
        };
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
