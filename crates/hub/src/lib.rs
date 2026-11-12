//! The hub: live service state in memory and the HTTP API on top of it.
//!
//! Current state is served from memory; history (state events, metric
//! points) comes from the database when one is configured.

mod api;
pub mod auth;
pub mod ca;
pub mod config;
mod link;
mod list;
mod locks;
mod nodes;
mod ops;
pub mod ssh;
mod stream;
pub mod totp;
mod ui;
mod view;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, SystemTime};
use tokio::sync::{broadcast, mpsc};
use watcher_config::{CheckDirs, Config};
use watcher_core::{CheckResult, HostState, ServiceState, StateType};
use watcher_exec::scheduler::{Control, Event, Job};
use watcher_exec::{DEFAULT_OUTPUT_LIMIT, ExecOptions};
use watcher_store::{HostSpec, ServiceSpec, Store, StoredState};

pub use auth::AuthSettings;
pub use config::{ConfigFiles, Reload};
pub use link::serve as serve_node_link;
pub use nodes::{NodeLink, TOKEN_TTL, check_name, join_command, new_join_token};
pub use ops::{DowntimeChange, OpenProblem};
pub use ssh::{StoreVault, remote};
/// Serves the web UI files and app shell; for routers outside the hub.
pub use ui::serve as serve_ui;

/// Live state of one service.
#[derive(Debug, Clone)]
struct LiveService {
    state: ServiceState,
    output: String,
    long_output: String,
    /// The full last result; `None` after a restart until the next check.
    result: Option<CheckResult>,
    duration: Duration,
}

/// Whether `id` is a service (not the host check) of a host that is DOWN
/// (HARD).
fn host_down(live: &Live, id: &str) -> bool {
    let config = &live.config;
    let Some(service) = config.service(id) else {
        return false;
    };
    if config.is_host_check(service) {
        return false;
    }
    config
        .host_check(config.host(service))
        .and_then(|s| live.services.get(&s.id))
        .is_some_and(|l| {
            HostState::from_check(l.state.state) == HostState::Down
                && l.state.state_type == StateType::Hard
        })
}

struct Live {
    config: Arc<Config>,
    /// When `config` was loaded.
    loaded: SystemTime,
    /// Keyed by `host/service`.
    services: HashMap<String, LiveService>,
    ops: ops::Ops,
}

/// A change pushed to stream subscribers, already serialized.
#[derive(Debug, Clone)]
enum Update {
    /// A service view after a check result.
    Result {
        /// `host/service`.
        id: Arc<str>,
        json: Arc<str>,
    },
    /// The configuration was reloaded; clients should refetch lists.
    Reload,
}

pub struct Hub {
    live: RwLock<Live>,
    updates: broadcast::Sender<Update>,
    control: mpsc::Sender<Control>,
    store: Option<Arc<Store>>,
    auth: AuthSettings,
    limiter: auth::Limiter,
    started: SystemTime,
    /// Set when the object files may be read and edited over the API.
    config_files: OnceLock<ConfigFiles>,
    /// Who has an object file open for editing; set with `config_files`.
    leases: OnceLock<locks::Leases>,
    /// Held while a config change is written and reloaded.
    config_lock: tokio::sync::Mutex<()>,
    /// `instance_name` from watcher.toml.
    instance: OnceLock<String>,
    /// Results per minute.
    results: Mutex<ops::Counter>,
    /// Hosts and services in a downtime at the last check, for
    /// [`Hub::downtime_changes`]; `None` before the first.
    downtime_seen: Mutex<Option<ops::InDowntime>>,
    /// Checks over SSH: test runs and the secrets API.
    ssh: OnceLock<ssh::Access>,
    /// What the open event streams get.
    watches: Arc<stream::Watches>,
    /// Counts changes of states, flags and the configuration.
    generation: AtomicU64,
    /// The summary of a generation, serialized.
    summary: Mutex<Option<(u64, Arc<str>)>>,
    /// Set on a hub: what nodes join with.
    link: OnceLock<NodeLink>,
    /// Nodes connected right now.
    nodes: nodes::Registry,
}

impl Hub {
    /// `restored` seeds the live state after a restart; `control` reaches
    /// the scheduler, for passive results. Authentication needs `store`.
    pub fn new(
        config: Arc<Config>,
        restored: HashMap<String, StoredState>,
        control: mpsc::Sender<Control>,
        store: Option<Arc<Store>>,
        auth: AuthSettings,
    ) -> Arc<Self> {
        let services = restored
            .into_iter()
            .map(|(id, stored)| {
                let live = LiveService {
                    state: stored.state,
                    output: stored.output,
                    long_output: stored.long_output,
                    result: None,
                    duration: Duration::ZERO,
                };
                (id, live)
            })
            .collect();
        let (updates, _) = broadcast::channel(1024);
        Arc::new(Hub {
            live: RwLock::new(Live {
                config,
                loaded: SystemTime::now(),
                services,
                ops: ops::Ops::default(),
            }),
            updates,
            control,
            store,
            auth,
            limiter: auth::Limiter::default(),
            started: SystemTime::now(),
            config_files: OnceLock::new(),
            leases: OnceLock::new(),
            config_lock: tokio::sync::Mutex::new(()),
            instance: OnceLock::new(),
            results: Mutex::new(ops::Counter::default()),
            downtime_seen: Mutex::new(None),
            ssh: OnceLock::new(),
            watches: Arc::default(),
            generation: AtomicU64::new(0),
            summary: Mutex::new(None),
            link: OnceLock::new(),
            nodes: nodes::Registry::default(),
        })
    }

    /// Changes with every state, flag or configuration change.
    fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    /// Lists and summaries are out of date.
    fn changed(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// The status summary of the current generation, serialized.
    fn summary_json(&self) -> Arc<str> {
        let generation = self.generation();
        let mut cached = self.summary.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((g, json)) = &*cached
            && *g == generation
        {
            return json.clone();
        }
        let summary = list::summary(&self.read(), generation);
        let json: Arc<str> = serde_json::to_string(&summary)
            .unwrap_or_else(|_| "{}".into())
            .into();
        *cached = Some((generation, json.clone()));
        json
    }

    fn read(&self) -> RwLockReadGuard<'_, Live> {
        self.live.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> RwLockWriteGuard<'_, Live> {
        self.live.write().unwrap_or_else(|e| e.into_inner())
    }

    /// The time zone of downtime group windows (default: the system's).
    pub fn set_timezone(&self, tz: jiff::tz::TimeZone) {
        self.write().ops.set_timezone(tz);
    }

    /// Names this instance in the API (`health`) and so in the web UI.
    pub fn set_instance_name(&self, name: String) {
        let _ = self.instance.set(name);
    }

    /// Runs test runs of `run_on = "host"` services over `pool`, and
    /// serves secrets and host keys from `vault` (`None` without database
    /// or data directory).
    pub fn enable_ssh(&self, pool: Arc<watcher_exec::ssh::Pool>, vault: Option<Arc<StoreVault>>) {
        let _ = self.ssh.set(ssh::Access { pool, vault });
    }

    /// Lets nodes join and connect (see [`serve_node_link`]), and serves
    /// them under `/api/v1/nodes`.
    pub fn enable_nodes(&self, link: NodeLink) {
        let _ = self.link.set(link);
    }

    /// Serves the object files under `/api/v1/config` (for admins).
    pub fn enable_config_editing(&self, files: ConfigFiles) {
        let _ = self.leases.set(locks::Leases::new(files.dir.clone()));
        let _ = self.config_files.set(files);
    }

    /// Drops edit leases that were not renewed; call every half minute.
    pub fn sweep_leases(&self) {
        if let Some(leases) = self.leases.get() {
            leases.sweep();
        }
    }

    /// The running configuration.
    pub fn config(&self) -> Arc<Config> {
        self.live
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .config
            .clone()
    }

    /// Installs a reloaded configuration and drops state of services that
    /// are gone.
    pub fn set_config(&self, config: Arc<Config>) {
        {
            let mut live = self.live.write().unwrap_or_else(|e| e.into_inner());
            let ids: std::collections::HashSet<&str> =
                config.services.iter().map(|s| s.id.as_str()).collect();
            live.services.retain(|id, _| ids.contains(id.as_str()));
            live.config = config;
            live.loaded = SystemTime::now();
        }
        // Host flags may now cover other services.
        self.sync_scheduler();
        self.changed();
        let _ = self.updates.send(Update::Reload);
    }

    /// Records a check result from the scheduler.
    pub fn record(&self, event: &Event) {
        self.results
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .add(SystemTime::now());
        if event.state.state == watcher_core::State::Ok {
            self.clear_acks(&event.id);
        }
        let (view, changed) = {
            let mut live = self.live.write().unwrap_or_else(|e| e.into_inner());
            let before = live.services.insert(
                event.id.clone(),
                LiveService {
                    state: event.state.clone(),
                    output: event.result.message.clone(),
                    long_output: event.result.long_output.clone(),
                    result: Some(event.result.clone()),
                    duration: event.duration,
                },
            );
            let changed = before.is_none_or(|b| {
                (b.state.state, b.state.state_type, b.state.flapping)
                    != (
                        event.state.state,
                        event.state.state_type,
                        event.state.flapping,
                    )
            });
            // Only make a view when someone listens for it.
            let wanted = self.updates.receiver_count() > 0 && self.watches.wanted(&event.id);
            let config = live.config.clone();
            let view = config.service(&event.id).filter(|_| wanted).map(|s| {
                let status = live.ops.status(&config, s, SystemTime::now());
                view::ServiceView::new(&config, s, live.services.get(&s.id), &status)
            });
            (view, changed)
        };
        if changed {
            self.changed();
        }
        if let Some(view) = view
            && let Ok(json) = serde_json::to_string(&view)
        {
            let id = event.id.as_str().into();
            let _ = self.updates.send(Update::Result {
                id,
                json: json.into(),
            });
        }
    }

    /// Whether notifications for service `id` are held back because its
    /// host is confirmed DOWN by the host check (Nagios semantics). The
    /// host check itself still notifies.
    pub fn host_down(&self, id: &str) -> bool {
        host_down(&self.read(), id)
    }

    pub fn router(self: Arc<Self>) -> axum::Router {
        api::router(self)
    }
}

/// Scheduler jobs for all services of `config`. Commands are looked up in
/// `check_dirs` first.
pub fn jobs(config: &Config, check_dirs: &CheckDirs) -> Vec<Job> {
    let path: Arc<std::ffi::OsStr> = check_dirs.search_path().as_os_str().into();
    config
        .services
        .iter()
        .map(|service| {
            let settings = &service.settings;
            Job {
                id: service.id.clone(),
                command: service.command.clone(),
                remote: remote(config, service),
                options: ExecOptions {
                    timeout: settings.timeout,
                    output_limit: DEFAULT_OUTPUT_LIMIT,
                    path: Some(path.clone()),
                },
                format: settings.format,
                thresholds: settings.thresholds.clone(),
                interval: settings.interval,
                retry_interval: settings.retry_interval,
                max_attempts: settings.max_attempts,
                host_check: config.is_host_check(service),
                freshness: settings.freshness,
                flap: settings.flap,
            }
        })
        .collect()
}

/// Hosts and services of `config` for [`Store::sync`].
pub fn specs(config: &Config) -> (Vec<HostSpec>, Vec<ServiceSpec>) {
    let hosts = config
        .hosts
        .iter()
        .map(|h| HostSpec {
            name: h.name.clone(),
            address: h.address.clone(),
            node: h.node.clone(),
            groups: h.groups.iter().map(|g| g.name.clone()).collect(),
            vars: serde_json::json!(h.vars),
        })
        .collect();
    let services = config
        .services
        .iter()
        .map(|s| ServiceSpec {
            host: config.host(s).name.clone(),
            name: s.name.clone(),
            check_hash: s.check_hash.clone(),
        })
        .collect();
    (hosts, services)
}
