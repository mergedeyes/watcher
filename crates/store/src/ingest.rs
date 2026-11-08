use crate::write::{STATE_FLUSH, StateBuffer};
use crate::{SeriesCache, Store};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use watcher_core::{CheckResult, ServiceState, StateChange};

/// Collect records for this long before writing a batch.
const BATCH_WINDOW: Duration = Duration::from_secs(1);
/// Upper bound for one transaction.
const MAX_BATCH: usize = 5_000;
/// Records kept while the database is unreachable. Beyond this the channel
/// fills up and senders should drop (`try_send`) rather than block. A durable
/// buffer comes with the sinks.
const MAX_PENDING: usize = 100_000;
const MAX_RETRY_DELAY: Duration = Duration::from_secs(60);

/// A check result to store. Serializable, so sinks can spool it on disk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// `host/service`.
    pub service: String,
    pub result: CheckResult,
    pub state: ServiceState,
    pub change: Option<StateChange>,
    /// How long the check ran.
    pub duration: Duration,
    /// `Some(true)` when the service started flapping, `Some(false)` when
    /// it stopped.
    #[serde(default)]
    pub flap: Option<bool>,
    /// The service gives its host UP/DOWN; its changes are also host events.
    #[serde(default)]
    pub host_check: bool,
}

/// Writes `records` in batches until the channel closes, then flushes.
///
/// `ids` maps `host/service` to database ids and is replaced on reload. On
/// database errors the batch is kept and retried with backoff. Current
/// state without a change is written every [`STATE_FLUSH`].
pub async fn ingest(
    store: Arc<Store>,
    ids: watch::Receiver<Arc<HashMap<String, i32>>>,
    records: mpsc::Receiver<Record>,
) {
    ingest_with(store, ids, records, STATE_FLUSH).await
}

/// [`ingest`] with another state flush interval, for tests.
#[doc(hidden)]
pub async fn ingest_with(
    store: Arc<Store>,
    ids: watch::Receiver<Arc<HashMap<String, i32>>>,
    mut records: mpsc::Receiver<Record>,
    state_flush: Duration,
) {
    let mut series = SeriesCache::default();
    let mut state = StateBuffer::new(state_flush);
    let mut pending: Vec<Record> = Vec::new();
    let mut retry_delay = Duration::from_secs(1);
    let mut closed = false;

    loop {
        if pending.is_empty() {
            // Idle: buffered state still goes out when due.
            let next = match state.due() {
                Some(at) => tokio::time::timeout_at(Instant::from_std(at), records.recv()).await,
                None => Ok(records.recv().await),
            };
            match next {
                Ok(Some(r)) => pending.push(r),
                Ok(None) => return flush_state(&store, &mut state).await,
                Err(_) => {
                    flush_state(&store, &mut state).await;
                    continue;
                }
            }
        }
        // Gather more for the batch window; after an error, this also
        // serves as the retry wait.
        let window = if retry_delay > Duration::from_secs(1) {
            retry_delay
        } else {
            BATCH_WINDOW
        };
        let deadline = Instant::now() + window;
        while !closed && pending.len() < MAX_PENDING {
            match tokio::time::timeout_at(deadline, records.recv()).await {
                Ok(Some(r)) => pending.push(r),
                Ok(None) => closed = true,
                Err(_) => break,
            }
        }

        let current_ids = ids.borrow().clone();
        while !pending.is_empty() {
            let n = pending.len().min(MAX_BATCH);
            match store
                .write(&pending[..n], &current_ids, &mut series, &mut state)
                .await
            {
                Ok(()) => {
                    pending.drain(..n);
                    retry_delay = Duration::from_secs(1);
                }
                Err(err) => {
                    tracing::warn!(
                        "storing {} results failed, retrying in {retry_delay:?}: {err}",
                        pending.len()
                    );
                    retry_delay = (retry_delay * 2).min(MAX_RETRY_DELAY);
                    if closed {
                        tracing::warn!(
                            "giving up on {} unstored results at shutdown",
                            pending.len()
                        );
                        return flush_state(&store, &mut state).await;
                    }
                    break;
                }
            }
        }
        if closed && pending.is_empty() {
            return flush_state(&store, &mut state).await;
        }
    }
}

/// Writes the buffered state; on errors, tries again after a full
/// interval.
async fn flush_state(store: &Store, state: &mut StateBuffer) {
    if let Err(err) = store.flush_state(state).await {
        tracing::warn!("storing the current state failed: {err}");
        state.postpone();
    }
}
