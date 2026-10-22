//! Runs jobs on their intervals and tracks their soft/hard state.
//!
//! First runs are spread over the interval by a hash of the job id, so a
//! restart does not start every check at once. While a problem is SOFT the
//! job runs on its retry interval. A reload keeps the state of jobs whose id
//! is unchanged.

use crate::{CommandLine, ExecOptions, Execution, run};
use std::cmp::Reverse;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BinaryHeap, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::{Semaphore, mpsc};
use tokio::time::Instant;
use watcher_core::{CheckResult, Format, ServiceState, StateChange, ThresholdSpec, evaluate};

/// A check to run periodically.
#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    /// Unique id, e.g. `host/check`.
    pub id: String,
    pub command: CommandLine,
    pub options: ExecOptions,
    pub format: Format,
    pub thresholds: BTreeMap<String, ThresholdSpec>,
    pub interval: Duration,
    pub retry_interval: Duration,
    pub max_attempts: u32,
}

/// The outcome of one run of a job.
#[derive(Debug, Clone)]
pub struct Event {
    pub id: String,
    pub result: CheckResult,
    pub execution: Execution,
    /// The state after this result.
    pub state: ServiceState,
    /// Set when the state or state type changed.
    pub change: Option<StateChange>,
}

struct Slot {
    job: Arc<Job>,
    state: Option<ServiceState>,
    /// When the job is due next; meaningless while it runs.
    due: Instant,
    running: bool,
}

struct Done {
    id: String,
    job: Arc<Job>,
    execution: Execution,
    result: CheckResult,
}

/// Runs `jobs` until `events` is closed. New job lists sent on `reload`
/// replace the current one.
pub async fn schedule(
    jobs: Vec<Job>,
    max_concurrent: usize,
    mut reload: mpsc::Receiver<Vec<Job>>,
    events: mpsc::Sender<Event>,
) {
    let permits = Arc::new(Semaphore::new(max_concurrent.max(1)));
    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<Done>();
    let mut slots: HashMap<String, Slot> = HashMap::new();
    let mut queue = BinaryHeap::new();
    let mut reload_open = true;
    replace_jobs(&mut slots, &mut queue, jobs);

    loop {
        let next_due = queue
            .peek()
            .map(|Reverse((due, _)): &Reverse<(Instant, String)>| *due);
        tokio::select! {
            () = sleep_until(next_due) => {
                let now = Instant::now();
                while let Some(Reverse((due, id))) = queue.peek().cloned() {
                    if due > now {
                        break;
                    }
                    queue.pop();
                    // Skip stale entries left by reloads and reschedules.
                    let Some(slot) = slots.get_mut(&id) else { continue };
                    if slot.running || slot.due != due {
                        continue;
                    }
                    slot.running = true;
                    let (job, permits, done) = (slot.job.clone(), permits.clone(), done_tx.clone());
                    tokio::spawn(async move {
                        let Ok(_permit) = permits.acquire_owned().await else { return };
                        let execution = run(&job.command, &job.options).await;
                        let result = evaluate(&execution.output, job.format, &job.thresholds);
                        let _ = done.send(Done { id, job, execution, result });
                    });
                }
            }
            Some(done) = done_rx.recv() => {
                let Some(event) = complete(&mut slots, &mut queue, done) else { continue };
                if events.send(event).await.is_err() {
                    return;
                }
            }
            jobs = reload.recv(), if reload_open => match jobs {
                Some(jobs) => replace_jobs(&mut slots, &mut queue, jobs),
                None => reload_open = false,
            },
            () = events.closed() => return,
        }
    }
}

async fn sleep_until(due: Option<Instant>) {
    match due {
        Some(due) => tokio::time::sleep_until(due).await,
        None => std::future::pending().await,
    }
}

/// Records a finished run and schedules the next one.
fn complete(
    slots: &mut HashMap<String, Slot>,
    queue: &mut BinaryHeap<Reverse<(Instant, String)>>,
    done: Done,
) -> Option<Event> {
    // The job may have been removed by a reload while it ran.
    let slot = slots.get_mut(&done.id)?;
    slot.running = false;
    let wall = SystemTime::now();
    let max_attempts = done.job.max_attempts;
    let (state, change) = match slot.state.as_mut() {
        Some(state) => {
            let change = state.apply(done.result.state, max_attempts, wall);
            (state.clone(), change)
        }
        None => {
            let (state, change) = ServiceState::first(done.result.state, max_attempts, wall);
            slot.state = Some(state.clone());
            (state, Some(change))
        }
    };

    let now = Instant::now();
    slot.due = if state.retrying() {
        now + slot.job.retry_interval
    } else {
        (slot.due + slot.job.interval).max(now)
    };
    queue.push(Reverse((slot.due, done.id.clone())));

    Some(Event {
        id: done.id,
        result: done.result,
        execution: done.execution,
        state,
        change,
    })
}

/// Installs a new job list, keeping state and timing of jobs that stay.
fn replace_jobs(
    slots: &mut HashMap<String, Slot>,
    queue: &mut BinaryHeap<Reverse<(Instant, String)>>,
    jobs: Vec<Job>,
) {
    let now = Instant::now();
    let mut old = std::mem::take(slots);
    for job in jobs {
        let slot = match old.remove(&job.id) {
            Some(mut slot) => {
                let interval_changed = slot.job.interval != job.interval;
                slot.job = Arc::new(job);
                if interval_changed && !slot.running {
                    slot.due = now + spread(&slot.job.id, slot.job.interval);
                    queue.push(Reverse((slot.due, slot.job.id.clone())));
                }
                slot
            }
            None => {
                let due = now + spread(&job.id, job.interval);
                queue.push(Reverse((due, job.id.clone())));
                Slot {
                    job: Arc::new(job),
                    state: None,
                    due,
                    running: false,
                }
            }
        };
        slots.insert(slot.job.id.clone(), slot);
    }
}

/// A stable offset in `[0, interval)` derived from the job id.
fn spread(id: &str, interval: Duration) -> Duration {
    let mut hasher = DefaultHasher::new();
    id.hash(&mut hasher);
    let millis = interval.as_millis().max(1) as u64;
    Duration::from_millis(hasher.finish() % millis)
}

#[cfg(test)]
mod tests {
    use super::*;
    use watcher_core::{State, StateType};

    fn job(id: &str, script: &str, interval_ms: u64) -> Job {
        Job {
            id: id.into(),
            command: CommandLine::Shell(script.into()),
            options: ExecOptions::default(),
            format: Format::Auto,
            thresholds: BTreeMap::new(),
            interval: Duration::from_millis(interval_ms),
            retry_interval: Duration::from_millis(interval_ms / 4),
            max_attempts: 3,
        }
    }

    async fn collect(rx: &mut mpsc::Receiver<Event>, n: usize) -> Vec<Event> {
        let mut out = Vec::new();
        while out.len() < n {
            let ev = tokio::time::timeout(Duration::from_secs(10), rx.recv())
                .await
                .expect("no event in time")
                .expect("scheduler stopped");
            out.push(ev);
        }
        out
    }

    #[test]
    fn spread_is_stable_and_bounded() {
        let i = Duration::from_secs(60);
        assert_eq!(spread("a/b", i), spread("a/b", i));
        assert!(spread("a/b", i) < i);
    }

    #[tokio::test]
    async fn failing_job_retries_until_hard() {
        let (_reload_tx, reload_rx) = mpsc::channel(1);
        let (tx, mut rx) = mpsc::channel(16);
        tokio::spawn(schedule(
            vec![job("h/fail", "echo broken; exit 2", 400)],
            4,
            reload_rx,
            tx,
        ));

        let events = collect(&mut rx, 3).await;
        let states: Vec<_> = events
            .iter()
            .map(|e| (e.state.state, e.state.state_type, e.state.attempt))
            .collect();
        assert_eq!(
            states,
            [
                (State::Critical, StateType::Soft, 1),
                (State::Critical, StateType::Soft, 2),
                (State::Critical, StateType::Hard, 3),
            ]
        );
        assert!(events[2].change.unwrap().hard);
        assert_eq!(events[0].result.message, "broken");
    }

    #[tokio::test]
    async fn reload_adds_jobs_and_keeps_state() {
        let (reload_tx, reload_rx) = mpsc::channel(1);
        let (tx, mut rx) = mpsc::channel(16);
        let a = job("h/a", "exit 0", 100);
        tokio::spawn(schedule(vec![a.clone()], 4, reload_rx, tx));

        let first = collect(&mut rx, 1).await;
        assert!(first[0].change.is_some());

        reload_tx
            .send(vec![a, job("h/b", "exit 1", 100)])
            .await
            .unwrap();
        let events = collect(&mut rx, 6).await;
        assert!(events.iter().any(|e| e.id == "h/b"));
        // `h/a` kept its state: no "first result" change after the reload.
        assert!(
            events
                .iter()
                .filter(|e| e.id == "h/a")
                .all(|e| e.change.is_none())
        );

        reload_tx.send(vec![]).await.unwrap();
        // Drain what was in flight, then nothing more arrives.
        while tokio::time::timeout(Duration::from_millis(300), rx.recv())
            .await
            .is_ok()
        {}
        assert!(
            tokio::time::timeout(Duration::from_millis(300), rx.recv())
                .await
                .is_err()
        );
    }
}
