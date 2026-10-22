//! Sinks: every result is also written to each configured sink's spool on
//! disk and flushed from there, so nothing is lost while a destination is
//! unreachable.

use anyhow::{Context, Result, anyhow};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::{info, warn};
use watcher_config::{Config, Flush, Sink, SinkKind};
use watcher_hub::specs;
use watcher_store::{Record, SeriesCache, Spool, Store, SyncMode};

/// Records sent per transaction.
const BATCH: usize = 2_000;
const MAX_BACKOFF: Duration = Duration::from_secs(300);

enum Msg {
    Record(Box<Record>),
    Config(Arc<Config>),
}

struct Running {
    def: Sink,
    tx: mpsc::Sender<Msg>,
    task: JoinHandle<()>,
    full_warned: bool,
}

pub struct Sinks {
    data_dir: Option<PathBuf>,
    instance: Option<String>,
    running: HashMap<String, Running>,
}

impl Sinks {
    /// Starts the sinks of `config`. Spools live in `data_dir/sinks/<name>`.
    pub async fn start(data_dir: Option<&Path>, config: &Arc<Config>) -> Result<Self> {
        let mut sinks = Sinks {
            data_dir: data_dir.map(Path::to_path_buf),
            instance: None,
            running: HashMap::new(),
        };
        sinks.reload(config).await?;
        Ok(sinks)
    }

    /// Queues a result for every sink. A sink that can't keep up even with
    /// its spool loses the result (with a warning) rather than stall checks.
    pub fn record(&mut self, record: &Record) {
        for (name, sink) in &mut self.running {
            match sink.tx.try_send(Msg::Record(Box::new(record.clone()))) {
                Ok(()) => sink.full_warned = false,
                Err(_) if !sink.full_warned => {
                    warn!("sink {name}: queue full, dropping results");
                    sink.full_warned = true;
                }
                Err(_) => {}
            }
        }
    }

    /// Starts new sinks, restarts changed ones and stops removed ones;
    /// unchanged sinks get the new config. Spools are kept.
    pub async fn reload(&mut self, config: &Arc<Config>) -> Result<()> {
        let names: Vec<String> = self.running.keys().cloned().collect();
        for name in names {
            let keep = config
                .sinks
                .iter()
                .any(|s| s.name == name && *s == self.running[&name].def);
            if keep {
                let _ = self.running[&name]
                    .tx
                    .send(Msg::Config(config.clone()))
                    .await;
            } else if let Some(old) = self.running.remove(&name) {
                // Wait for it to let go of its spool before a new one opens it.
                drop(old.tx);
                let _ = old.task.await;
            }
        }
        for def in &config.sinks {
            if self.running.contains_key(&def.name) {
                continue;
            }
            let data_dir = self.data_dir.clone().ok_or_else(|| {
                anyhow!(
                    "sink {}: needs data_dir from watcher.toml for its buffer",
                    def.name
                )
            })?;
            if self.instance.is_none() {
                self.instance = Some(instance_id(&data_dir)?);
            }
            let source = format!(
                "{}/{}",
                self.instance.as_deref().unwrap_or_default(),
                def.name
            );
            let spool = Spool::open(&data_dir.join("sinks").join(&def.name), def.max_buffer)
                .with_context(|| format!("sink {}: cannot open its buffer", def.name))?;
            let (tx, rx) = mpsc::channel(10_000);
            let task = tokio::spawn(run(def.clone(), spool, source, config.clone(), rx));
            self.running.insert(
                def.name.clone(),
                Running {
                    def: def.clone(),
                    tx,
                    task,
                    full_warned: false,
                },
            );
        }
        Ok(())
    }

    /// Stops all sinks. Unsent data stays in the spools for the next start.
    pub async fn shutdown(self) {
        for (_, sink) in self.running {
            drop(sink.tx);
            let _ = tokio::time::timeout(Duration::from_secs(5), sink.task).await;
        }
    }
}

/// A stable id of this instance, so a destination can tell senders apart.
fn instance_id(data_dir: &Path) -> Result<String> {
    let path = data_dir.join("instance-id");
    if let Ok(id) = std::fs::read_to_string(&path) {
        let id = id.trim();
        if !id.is_empty() {
            return Ok(id.to_string());
        }
    }
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).map_err(|e| anyhow!("random: {e}"))?;
    let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    std::fs::create_dir_all(data_dir)?;
    std::fs::write(&path, format!("{id}\n"))
        .with_context(|| format!("cannot write {}", path.display()))?;
    Ok(id)
}

/// A connected destination.
struct Remote {
    store: Store,
    ids: HashMap<String, i32>,
    series: SeriesCache,
}

async fn run(
    def: Sink,
    mut spool: Spool,
    source: String,
    mut config: Arc<Config>,
    mut rx: mpsc::Receiver<Msg>,
) {
    let interval = match def.flush {
        Flush::Interval(d) => d,
        Flush::Realtime => Duration::from_secs(1),
    };
    let mut remote: Option<Remote> = None;
    let mut synced = false;
    let mut backoff = Duration::from_secs(5);
    let mut failing = false;
    // Data left from the last run goes out soon.
    let mut next_flush = Instant::now()
        + if spool.pending() > 0 {
            Duration::from_secs(2)
        } else {
            interval
        };
    if spool.pending() > 0 {
        info!(
            "sink {}: {} buffered results from before",
            def.name,
            spool.pending()
        );
    }

    loop {
        tokio::select! {
            msg = rx.recv() => {
                let Some(msg) = msg else { return };
                let mut batch = Vec::new();
                let mut handle = |msg: Msg, batch: &mut Vec<Record>| match msg {
                    Msg::Record(r) => batch.push(*r),
                    Msg::Config(c) => {
                        config = c;
                        synced = false;
                    }
                };
                handle(msg, &mut batch);
                while batch.len() < 1_000 {
                    match rx.try_recv() {
                        Ok(msg) => handle(msg, &mut batch),
                        Err(_) => break,
                    }
                }
                match spool.append(&batch) {
                    Ok(0) => {}
                    Ok(dropped) => warn!("sink {}: buffer full, dropped {dropped} oldest results", def.name),
                    Err(err) => warn!("sink {}: cannot write buffer: {err}", def.name),
                }
            }
            () = tokio::time::sleep_until(next_flush) => {
                match flush(&def, &mut spool, &mut remote, &mut synced, &config, &source).await {
                    Ok(sent) => {
                        if failing {
                            info!("sink {}: delivering again", def.name);
                        }
                        if sent > 0 {
                            tracing::debug!("sink {}: sent {sent} results", def.name);
                        }
                        failing = false;
                        backoff = Duration::from_secs(5);
                        next_flush = Instant::now() + interval;
                    }
                    Err(err) => {
                        warn!(
                            "sink {}: {err:#}; retry in {}s, {} results buffered",
                            def.name,
                            backoff.as_secs(),
                            spool.pending()
                        );
                        failing = true;
                        remote = None;
                        next_flush = Instant::now() + backoff;
                        backoff = (backoff * 2).min(MAX_BACKOFF);
                    }
                }
            }
        }
    }
}

/// Sends everything buffered. Returns the number of results sent.
async fn flush(
    def: &Sink,
    spool: &mut Spool,
    remote: &mut Option<Remote>,
    synced: &mut bool,
    config: &Config,
    source: &str,
) -> Result<u64> {
    if spool.pending() == 0 {
        return Ok(0);
    }
    if remote.is_none() {
        let SinkKind::Postgres { url, password_file } = &def.kind;
        let password = match password_file {
            Some(file) => Some(
                std::fs::read_to_string(file)
                    .with_context(|| format!("cannot read {}", file.display()))?
                    .trim_end_matches(['\r', '\n'])
                    .to_string(),
            ),
            None => None,
        };
        let store = Store::connect_with(url, password.as_deref())
            .await
            .context("connect")?;
        info!("sink {}: connected ({})", def.name, store.backend());
        *remote = Some(Remote {
            store,
            ids: HashMap::new(),
            series: SeriesCache::default(),
        });
        *synced = false;
    }
    let r = remote.as_mut().expect("connected above");
    if !*synced {
        // Other instances may write to the same database: never mark
        // their hosts removed.
        let (hosts, services) = specs(config);
        r.ids = r
            .store
            .sync_with(&hosts, &services, SyncMode::Additive)
            .await
            .context("sync")?;
        *synced = true;
    }

    let mut sent = 0;
    loop {
        let batch: Vec<(u64, Record)> = spool.read(BATCH).context("read buffer")?;
        let Some(&(last, _)) = batch.last() else {
            break;
        };
        r.store
            .write_sink(source, &batch, &r.ids, &mut r.series)
            .await
            .context("write")?;
        spool.ack(last).context("buffer")?;
        sent += batch.len() as u64;
    }
    Ok(sent)
}
