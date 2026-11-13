use crate::config_cmd;
use crate::sinks::Sinks;
use anyhow::{Context, Result, bail};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};
use watcher_config::{
    CheckDirs, Config, ConfigMode, InstanceRole, RunOn, Severity, files, load_dir,
};
use watcher_core::StateType;
use watcher_exec::scheduler::{Control, Event, schedule};
use watcher_exec::ssh::Pool;
use watcher_hub::{AuthSettings, ConfigFiles, Hub, StoreVault, jobs, specs};
use watcher_notify::{
    DowntimeChange, Message, Notifier, OpenProblem, Repeats, Sent, setup_warnings,
};
use watcher_store::{Record, SentNotification, Store, StoredState, ingest};

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

impl Args {
    /// The defaults, for `path`.
    pub fn new(path: PathBuf) -> Self {
        Args {
            path: Some(path),
            all: false,
            max_concurrent: 64,
            database: None,
            no_database: false,
            no_auth: false,
            listen: None,
        }
    }
}

pub async fn run(mut args: Args) -> Result<ExitCode> {
    // A watcher.toml that doesn't exist yet: set it up in the browser.
    if let Some(path) = &args.path
        && path.extension().is_some_and(|e| e == "toml")
        && !path.exists()
    {
        let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
        let dir = dir.unwrap_or(Path::new(".")).to_path_buf();
        let listen = args
            .listen
            .unwrap_or(SocketAddr::from(([127, 0, 0, 1], 8080)));
        args.path = Some(crate::setup::run(&dir, listen).await?);
        // The address chosen in setup applies from now on.
        args.listen = None;
    }
    let (bootstrap, conf_dir) = config_cmd::locate(args.path.as_deref())?;
    // A node only keeps the link to its hub.
    if let Some(b) = bootstrap.as_ref().filter(|b| b.role == InstanceRole::Node) {
        return crate::node_cmd::run_node(b).await;
    }
    let node_link = match bootstrap.as_ref().filter(|b| b.role == InstanceRole::Hub) {
        Some(b) => Some(crate::node_cmd::link_plan(b)?),
        None => None,
    };
    let check_dirs = config_cmd::check_dirs(bootstrap.as_ref(), &conf_dir);
    let Some(config) = load(&conf_dir, &check_dirs) else {
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
    // With ACME, the task that gets the certificates starts once the
    // server listens.
    let (tls, acme) = match bootstrap
        .as_ref()
        .and_then(|b| b.tls.as_ref().map(|t| (t, &b.data_dir)))
    {
        Some((t, data_dir)) => match &t.acme {
            Some(settings) => {
                let (config, acme) = crate::acme::setup(settings, data_dir, listen)?;
                (Some(config), Some(acme))
            }
            None => {
                let names = crate::tls::names(listen, public_url.as_deref());
                (Some(crate::tls::config(t, data_dir, &names).await?), None)
            }
        },
        None => (None, None),
    };
    // Behind a TLS proxy (public_url) or serving TLS itself.
    let https = tls.is_some()
        || public_url
            .as_deref()
            .is_some_and(|u| u.starts_with("https://"));
    let instance = bootstrap.as_ref().and_then(|b| b.instance_name.clone());
    let timezone = bootstrap.as_ref().and_then(|b| b.timezone.clone());
    let notifier = Arc::new(
        Notifier::new(smtp.as_ref(), public_url)
            .context("invalid [smtp] settings")?
            .with_instance(instance.clone()),
    );
    for warning in setup_warnings(&config, &notifier) {
        warn!("{warning}");
    }
    let (config_tx, config_rx) = watch::channel(config.clone());
    let (notify_tx, notify_rx) = mpsc::channel(1000);

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
    // The notifier goes on with the repeat clock from the log.
    let (repeats, log) = match &storage {
        Some(s) => {
            let (repeats, log) = notification_log(s.store.clone())
                .await
                .context("cannot read the notification log")?;
            (repeats, Some(log))
        }
        None => (Repeats::default(), None),
    };
    // Downtime windows and time periods use the same zone.
    let tz = match &timezone {
        // Checked when watcher.toml was loaded.
        Some(name) => jiff::tz::TimeZone::get(name)?,
        None => jiff::tz::TimeZone::system(),
    };
    tokio::spawn(watcher_notify::run(
        notifier.clone(),
        config_rx,
        notify_rx,
        repeats,
        log,
        tz.clone(),
    ));

    let initial = restored
        .iter()
        .map(|(id, s)| (id.clone(), s.state.clone()))
        .collect();

    // Checks over SSH: secrets in the database, sealed with data_dir/secret.key.
    let ssh_vault = match (&storage, &data_dir) {
        (Some(s), Some(dir)) => Some(Arc::new(StoreVault::new(s.store.clone(), dir.clone()))),
        _ => None,
    };
    let ssh = Arc::new(match (&ssh_vault, &storage) {
        (Some(vault), _) => Pool::new(vault.clone()),
        (None, None) => Pool::unavailable("checks over SSH need a database for their secrets"),
        (None, Some(_)) => Pool::unavailable("checks over SSH need watcher.toml (data_dir)"),
    });
    if ssh_vault.is_none()
        && config
            .services
            .iter()
            .any(|s| s.settings.run_on == RunOn::Host)
    {
        warn!("services with run_on = \"host\" can't run: SSH needs a database and watcher.toml");
    }

    let (control, control_rx) = mpsc::channel(64);
    let (events_tx, mut events) = mpsc::channel(256);
    let scheduler = tokio::spawn(schedule(
        jobs(&config, &check_dirs),
        initial,
        args.max_concurrent,
        ssh.clone(),
        control_rx,
        events_tx,
    ));

    let store = storage.as_ref().map(|s| s.store.clone());
    let auth = auth_settings(args.no_auth, listen, https, store.as_deref()).await?;
    let hub = Hub::new(config.clone(), restored, control.clone(), store, auth);
    hub.enable_ssh(ssh, ssh_vault);
    if let Some(name) = instance {
        hub.set_instance_name(name);
    }
    hub.set_timezone(tz);
    // Acknowledgements, downtimes, muted and disabled checks.
    hub.load_ops()
        .await
        .context("cannot load acknowledgements and downtimes")?;
    let (reload_tx, mut reloads) = mpsc::channel(4);
    hub.enable_config_editing(ConfigFiles {
        dir: conf_dir.clone(),
        mode,
        reload: reload_tx,
        check_dirs: check_dirs.clone(),
    });
    // Discovery keeps what it finds in the database.
    if storage.is_some() {
        tokio::spawn(watcher_hub::discovery::run(hub.clone()));
    }
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
    let link = match node_link {
        Some(plan) => Some(crate::node_cmd::start_link(&hub, plan, storage.is_some())?),
        None => None,
    };
    let scheme = if tls.is_some() { "https" } else { "http" };
    let server = serve(listen, hub.clone().router(), tls)?;
    if let Some(acme) = acme {
        tokio::spawn(acme.run());
    }
    info!(
        "started with {} services from {}; web UI on {scheme}://{listen}",
        config.services.len(),
        conf_dir.display()
    );
    let dirs: Vec<String> = check_dirs
        .dirs()
        .iter()
        .map(|d| d.display().to_string())
        .collect();
    info!("check directories: {}", dirs.join(", "));

    let mut running = Running {
        storage,
        sinks,
        control: &control,
        notifier: &notifier,
        notify_tx: &notify_tx,
        config_tx: &config_tx,
        hub: &hub,
        check_dirs: &check_dirs,
        host_checks: host_checks(&config),
    };
    // Downtimes starting and ending (windows are per minute).
    let mut downtimes = tokio::time::interval(Duration::from_secs(15));
    // Open problems, for repeated notifications (`repeat` is at least 1m).
    let mut repeats = tokio::time::interval(Duration::from_secs(30));
    // Edit leases of closed browser tabs.
    let mut leases = tokio::time::interval(Duration::from_secs(30));
    let mut hangup = signal(SignalKind::hangup())?;
    let mut terminate = signal(SignalKind::terminate())?;
    loop {
        tokio::select! {
            Some(event) = events.recv() => running.record(event, args.all),
            _ = downtimes.tick() => running.downtimes().await,
            _ = repeats.tick() => running.repeats(),
            _ = leases.tick() => hub.sweep_leases(),
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
    if let Some(link) = link {
        link.abort();
    }
    scheduler.abort();
    if let Some(storage) = running.storage {
        storage.shutdown().await;
    }
    running.sinks.shutdown().await;
    Ok(ExitCode::SUCCESS)
}

/// How long the notification log keeps entries.
const NOTIFICATION_LOG_KEEP: Duration = Duration::from_secs(90 * 86400);

/// The repeat clock from the notification log, and a sender for new
/// entries: a task writes them (in batches) and deletes old ones hourly.
async fn notification_log(store: Arc<Store>) -> anyhow::Result<(Repeats, mpsc::Sender<Sent>)> {
    let start = store.notification_log_start().await?;
    // Open problems older than this have their clock started anew.
    let history = store.last_problems(NOTIFICATION_LOG_KEEP).await?;
    let (tx, mut rx) = mpsc::channel::<Sent>(1000);
    tokio::spawn(async move {
        let mut purge = tokio::time::interval(Duration::from_secs(3600));
        loop {
            tokio::select! {
                entry = rx.recv() => {
                    let Some(entry) = entry else { break };
                    let mut batch = vec![log_row(entry)];
                    while let Ok(more) = rx.try_recv() {
                        batch.push(log_row(more));
                    }
                    if let Err(err) = store.add_notifications(&batch).await {
                        warn!("cannot write {} entries to the notification log: {err}", batch.len());
                    }
                }
                _ = purge.tick() => match store.purge_notifications(NOTIFICATION_LOG_KEEP).await {
                    Ok(0) => {}
                    Ok(n) => info!("removed {n} old entries from the notification log"),
                    Err(err) => warn!("cannot clean up the notification log: {err}"),
                },
            }
        }
    });
    Ok((Repeats::restore(history, start), tx))
}

fn log_row(s: Sent) -> SentNotification {
    let n = &s.notification;
    SentNotification {
        ts: s.time,
        host: n.host.clone(),
        service: n.service.clone(),
        host_check: n.host_check,
        kind: n.kind.as_str().to_ascii_lowercase(),
        state: n.state_name().to_ascii_lowercase(),
        repeat: n.repeat,
        since: n.since,
        contact: s.contact,
        channel: s.channel.to_string(),
        error: s.error,
    }
}

/// Where results go, and what a reload updates.
struct Running<'a> {
    storage: Option<Storage>,
    sinks: Sinks,
    control: &'a mpsc::Sender<Control>,
    notifier: &'a Notifier,
    notify_tx: &'a mpsc::Sender<Message>,
    config_tx: &'a watch::Sender<Arc<Config>>,
    hub: &'a Hub,
    check_dirs: &'a CheckDirs,
    /// Ids of the services that give their host UP/DOWN.
    host_checks: HashSet<String>,
}

impl Running<'_> {
    fn record(&mut self, event: Event, all: bool) {
        print_event(&event, all);
        self.hub.record(&event);
        if event.change.is_some_and(|c| c.hard) || event.flap.is_some() {
            if let Some(why) = self.hub.suppressed(&event.id, event.state.state) {
                info!("no notification for {}: {why}", event.id);
            } else if self
                .notify_tx
                .try_send(Message::Result(Box::new(event.clone())))
                .is_err()
            {
                warn!(
                    "notification queue is full; dropped the notification for {}",
                    event.id
                );
            }
        }
        let record = Record {
            host_check: self.host_checks.contains(&event.id),
            service: event.id,
            result: event.result,
            state: event.state,
            change: event.change,
            duration: event.duration,
            flap: event.flap,
        };
        self.sinks.record(&record);
        if let Some(storage) = &mut self.storage {
            storage.record(record);
        }
    }

    /// Records and notifies downtimes that started or ended.
    async fn downtimes(&mut self) {
        for c in self.hub.downtime_changes().await {
            let id = match &c.service {
                Some(s) => format!("{}/{s}", c.host),
                None => c.host.clone(),
            };
            let what = if c.started { "started" } else { "ended" };
            info!("downtime {what}: {id} ({})", c.what);
            if let Some(why) = self.hub.downtime_suppressed(&c.host, c.service.as_deref()) {
                info!("no notification for {id}: {why}");
                continue;
            }
            let change = DowntimeChange {
                host: c.host,
                service: c.service,
                started: c.started,
                state: c.state,
                what: c.what,
                time: std::time::SystemTime::now(),
            };
            if self.notify_tx.try_send(Message::Downtime(change)).is_err() {
                warn!("notification queue is full; dropped the downtime notification for {id}");
            }
        }
    }

    /// Hands the open problems to the notifier, which sends the repeats
    /// that are due.
    fn repeats(&self) {
        let open = self
            .hub
            .open_problems()
            .into_iter()
            .map(|p| OpenProblem {
                id: p.id,
                state: p.state,
                attempts: p.attempts,
                output: p.output,
                long_output: p.long_output,
                last_check: p.last_check,
                since: p.since,
                suppressed: p.suppressed.is_some(),
            })
            .collect();
        if self.notify_tx.try_send(Message::Problems(open)).is_err() {
            warn!("notification queue is full; repeated notifications are late");
        }
    }

    /// Loads the object files and switches to them. On errors the running
    /// configuration stays.
    async fn reload(&mut self, conf_dir: &Path, by: &str) -> Result<(), String> {
        info!("reloading ({by})");
        let Some(config) = load(conf_dir, self.check_dirs) else {
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
            .send(Control::Reload(jobs(&config, self.check_dirs)))
            .await
            .is_err()
        {
            return Err("the scheduler is not running".into());
        }
        for warning in setup_warnings(&config, self.notifier) {
            warn!("{warning}");
        }
        self.host_checks = host_checks(&config);
        let config = Arc::new(config);
        if let Err(err) = self.sinks.reload(&config).await {
            error!("{err:#}");
        }
        self.config_tx.send_replace(config.clone());
        self.hub.set_config(config);
        Ok(())
    }
}

fn host_checks(config: &Config) -> HashSet<String> {
    config
        .services
        .iter()
        .filter(|s| config.is_host_check(s))
        .map(|s| s.id.clone())
        .collect()
}

/// Hashes of the object files, to tell whether anything changed.
fn snapshot(conf_dir: &Path) -> Vec<(String, String)> {
    files::list(conf_dir)
        .unwrap_or_default()
        .into_iter()
        .map(|f| (f.path, f.hash))
        .collect()
}

/// Serves `router` on `listen`, over TLS when `tls` is set. Fails early if
/// the address cannot be bound.
pub(crate) fn serve(
    listen: SocketAddr,
    router: axum::Router,
    tls: Option<axum_server::tls_rustls::RustlsConfig>,
) -> Result<JoinHandle<std::io::Result<()>>> {
    let listener = std::net::TcpListener::bind(listen)
        .with_context(|| format!("cannot listen on {listen}"))?;
    listener.set_nonblocking(true)?;
    Ok(match tls {
        Some(tls) => {
            let server = axum_server::from_tcp_rustls(listener, tls)?;
            tokio::spawn(server.serve(router.into_make_service()))
        }
        None => {
            let listener = tokio::net::TcpListener::from_std(listener)?;
            tokio::spawn(axum::serve(listener, router).into_future())
        }
    })
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

/// Loads the config, logging diagnostics (including commands not found in
/// `check_dirs`). `None` if it has errors.
fn load(conf_dir: &Path, check_dirs: &CheckDirs) -> Option<Config> {
    let (config, diagnostics) = match load_dir(conf_dir) {
        Ok(mut loaded) => {
            let missing = check_dirs.missing_commands(&loaded.config);
            loaded.warnings.extend(missing);
            (Some(loaded.config), loaded.warnings)
        }
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
