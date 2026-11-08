//! The event stream (`GET /stream`): check results, reloads and status
//! summaries as server-sent events.
//!
//! Without `watch` a stream gets every result. With `watch=true` it starts
//! with a `hello` event carrying its id and gets only results of the
//! services and hosts set with `PUT /stream/{id}`: the rows a page shows.
//! Every stream gets a `summary` when states changed, at most once a second.

use crate::api::ApiError;
use crate::{Hub, Update};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use serde::Deserialize;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::time::MissedTickBehavior;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::{BroadcastStream, IntervalStream};
use tokio_stream::{Stream, StreamExt};

/// How often a stream looks for a new summary.
const SUMMARY_EVERY: Duration = Duration::from_secs(1);
/// Entries a watch may have.
const MAX_WATCH: usize = 10_000;

/// What one stream gets results of.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct Watch {
    /// `host/service` ids.
    #[serde(default)]
    services: HashSet<String>,
    /// Hosts: all their services.
    #[serde(default)]
    hosts: HashSet<String>,
}

impl Watch {
    fn wants(&self, id: &str) -> bool {
        self.services.contains(id)
            || id
                .split_once('/')
                .is_some_and(|(host, _)| self.hosts.contains(host))
    }
}

/// The open streams' watches, to send and serialize only wanted results.
#[derive(Debug, Default)]
pub(crate) struct Watches {
    watches: Mutex<HashMap<String, Arc<RwLock<Watch>>>>,
    /// Open streams without a watch: they get everything.
    unfiltered: AtomicUsize,
}

impl Watches {
    /// Whether any open stream gets results of service `id`.
    pub fn wanted(&self, id: &str) -> bool {
        self.unfiltered.load(Ordering::Relaxed) > 0
            || self
                .watches
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .values()
                .any(|w| w.read().unwrap_or_else(|e| e.into_inner()).wants(id))
    }

    fn open(self: &Arc<Self>, watch: bool) -> Registration {
        if !watch {
            self.unfiltered.fetch_add(1, Ordering::Relaxed);
            return Registration {
                watches: self.clone(),
                id: None,
                watch: None,
            };
        }
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).expect("the OS random number generator works");
        let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let watch = Arc::new(RwLock::new(Watch::default()));
        self.watches
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), watch.clone());
        Registration {
            watches: self.clone(),
            id: Some(id),
            watch: Some(watch),
        }
    }
}

/// A stream's place in [`Watches`], given up when the stream ends.
struct Registration {
    watches: Arc<Watches>,
    id: Option<String>,
    watch: Option<Arc<RwLock<Watch>>>,
}

impl Registration {
    fn wants(&self, id: &str) -> bool {
        self.watch
            .as_ref()
            .is_none_or(|w| w.read().unwrap_or_else(|e| e.into_inner()).wants(id))
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        match &self.id {
            Some(id) => {
                self.watches
                    .watches
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(id);
            }
            None => {
                self.watches.unfiltered.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct StreamQuery {
    /// Only results set with `PUT /stream/{id}`.
    #[serde(default)]
    watch: bool,
}

pub(crate) async fn stream(
    State(hub): State<Arc<Hub>>,
    Query(q): Query<StreamQuery>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let registration = hub.watches.open(q.watch);
    let hello = registration.id.as_ref().map(|id| {
        Event::default()
            .event("hello")
            .data(json!({ "id": id }).to_string())
    });
    let updates = BroadcastStream::new(hub.updates.subscribe()).filter_map(move |msg| match msg {
        Ok(Update::Result { id, json }) => registration
            .wants(&id)
            .then(|| Event::default().event("result").data(&*json)),
        Ok(Update::Reload) => Some(Event::default().event("reload").data("{}")),
        Err(BroadcastStreamRecvError::Lagged(n)) => {
            Some(Event::default().event("lagged").data(n.to_string()))
        }
    });
    let mut every = tokio::time::interval(SUMMARY_EVERY);
    every.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut sent = None;
    let summaries = IntervalStream::new(every).filter_map(move |_| {
        let generation = hub.generation();
        if sent == Some(generation) {
            return None;
        }
        sent = Some(generation);
        Some(Event::default().event("summary").data(&*hub.summary_json()))
    });
    let events = tokio_stream::iter(hello)
        .chain(updates.merge(summaries))
        .map(Ok);
    Sse::new(events).keep_alive(KeepAlive::default())
}

/// `PUT /stream/{id}`: the services (`host/service`) and hosts a stream
/// opened with `watch=true` gets results of, replacing the previous set.
pub(crate) async fn set_watch(
    State(hub): State<Arc<Hub>>,
    Path(id): Path<String>,
    Json(watch): Json<Watch>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if watch.services.len() + watch.hosts.len() > MAX_WATCH {
        return Err(ApiError::BadRequest(format!(
            "at most {MAX_WATCH} services and hosts"
        )));
    }
    let watches = hub
        .watches
        .watches
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let entry = watches
        .get(&id)
        .ok_or_else(|| ApiError::NotFound(format!("no stream {id:?}")))?;
    let counts = json!({ "services": watch.services.len(), "hosts": watch.hosts.len() });
    *entry.write().unwrap_or_else(|e| e.into_inner()) = watch;
    Ok(Json(counts))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_matches_services_and_hosts() {
        let watch = Watch {
            services: HashSet::from(["web01/http".to_string()]),
            hosts: HashSet::from(["db01".to_string()]),
        };
        assert!(watch.wants("web01/http"));
        assert!(!watch.wants("web01/ping"));
        assert!(watch.wants("db01/backup"));
        assert!(!watch.wants("db011/backup"));
    }

    #[test]
    fn registrations_come_and_go() {
        let watches = Arc::new(Watches::default());
        assert!(!watches.wanted("a/b"));
        let all = watches.open(false);
        assert!(watches.wanted("a/b"));
        drop(all);
        assert!(!watches.wanted("a/b"));
        let one = watches.open(true);
        assert!(!watches.wanted("a/b"));
        one.watch
            .as_ref()
            .unwrap()
            .write()
            .unwrap()
            .hosts
            .insert("a".into());
        assert!(watches.wanted("a/b"));
        drop(one);
        assert!(watches.watches.lock().unwrap().is_empty());
    }
}
