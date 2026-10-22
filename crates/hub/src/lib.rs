//! The hub: live service state in memory and the HTTP API on top of it.
//!
//! Current state is served from memory; history (state events, metric
//! points) comes from the database when one is configured.

mod api;
pub mod auth;
pub mod config;
mod ui;
mod view;

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, SystemTime};
use tokio::sync::{broadcast, mpsc};
use watcher_config::{CheckDirs, Config};
use watcher_core::{CheckResult, HostState, ServiceState, StateType};
use watcher_exec::scheduler::{Control, Event, Job};
use watcher_exec::{DEFAULT_OUTPUT_LIMIT, ExecOptions};
use watcher_store::{HostSpec, ServiceSpec, Store, StoredState};

pub use auth::AuthSettings;
pub use config::{ConfigFiles, Reload};
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

struct Live {
    config: Arc<Config>,
    /// Keyed by `host/service`.
    services: HashMap<String, LiveService>,
}

/// A change pushed to stream subscribers, already serialized.
#[derive(Debug, Clone)]
enum Update {
    /// A service view after a check result.
    Result(Arc<str>),
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
    /// Held while a config change is written and reloaded.
    config_lock: tokio::sync::Mutex<()>,
    /// `instance_name` from watcher.toml.
    instance: OnceLock<String>,
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
            live: RwLock::new(Live { config, services }),
            updates,
            control,
            store,
            auth,
            limiter: auth::Limiter::default(),
            started: SystemTime::now(),
            config_files: OnceLock::new(),
            config_lock: tokio::sync::Mutex::new(()),
            instance: OnceLock::new(),
        })
    }

    /// Names this instance in the API (`health`) and so in the web UI.
    pub fn set_instance_name(&self, name: String) {
        let _ = self.instance.set(name);
    }

    /// Serves the object files under `/api/v1/config` (for admins).
    pub fn enable_config_editing(&self, files: ConfigFiles) {
        let _ = self.config_files.set(files);
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
        }
        let _ = self.updates.send(Update::Reload);
    }

    /// Records a check result from the scheduler.
    pub fn record(&self, event: &Event) {
        let view = {
            let mut live = self.live.write().unwrap_or_else(|e| e.into_inner());
            live.services.insert(
                event.id.clone(),
                LiveService {
                    state: event.state.clone(),
                    output: event.result.message.clone(),
                    long_output: event.result.long_output.clone(),
                    result: Some(event.result.clone()),
                    duration: event.duration,
                },
            );
            let config = live.config.clone();
            config
                .services
                .iter()
                .find(|s| s.id == event.id)
                .map(|s| view::ServiceView::new(&config, s, live.services.get(&s.id)))
        };
        // Only serialize when someone listens.
        if let Some(view) = view
            && self.updates.receiver_count() > 0
            && let Ok(json) = serde_json::to_string(&view)
        {
            let _ = self.updates.send(Update::Result(json.into()));
        }
    }

    /// Whether notifications for service `id` are held back because its
    /// host is confirmed DOWN by the host check (Nagios semantics). The
    /// host check itself still notifies.
    pub fn host_down(&self, id: &str) -> bool {
        let live = self.live.read().unwrap_or_else(|e| e.into_inner());
        let config = &live.config;
        let Some(service) = config.services.iter().find(|s| s.id == id) else {
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
            let check = config.check(service);
            Job {
                id: service.id.clone(),
                command: service.command.clone(),
                options: ExecOptions {
                    timeout: check.timeout,
                    output_limit: DEFAULT_OUTPUT_LIMIT,
                    path: Some(path.clone()),
                },
                format: check.format,
                thresholds: check.thresholds.clone(),
                interval: check.interval,
                retry_interval: check.retry_interval,
                max_attempts: check.max_attempts,
                freshness: check.freshness,
                flap: check.flap,
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
            tags: h.tags.clone(),
            vars: h.vars.clone(),
        })
        .collect();
    let services = config
        .services
        .iter()
        .map(|s| ServiceSpec {
            host: config.host(s).name.clone(),
            name: config.check(s).name.clone(),
            check_hash: s.check_hash.clone(),
        })
        .collect();
    (hosts, services)
}
