//! Runs jobs on their intervals and tracks their soft/hard state.
//!
//! First runs are spread over the interval by a hash of the job id, so a
//! restart does not start every check at once. While a problem is SOFT the
//! job runs on its retry interval. A reload keeps the state of jobs whose id
//! is unchanged.
//!
//! Passive jobs have no command: their results are submitted from outside
//! (the HTTP API) and go through the same state machine. With `freshness`
//! set, a missing result turns them UNKNOWN.

use crate::{CommandLine, ExecOptions, run};
use std::cmp::Reverse;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BinaryHeap, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::{Semaphore, mpsc, oneshot};
use tokio::time::Instant;
use watcher_core::{
    CheckResult, Format, RawOutput, ServiceState, StateChange, Termination, ThresholdSpec, evaluate,
};

/// A check to run periodically.
#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    /// Unique id, e.g. `host/check`.
    pub id: String,
    /// `None` for passive jobs.
    pub command: Option<CommandLine>,
    pub options: ExecOptions,
    pub format: Format,
    pub thresholds: BTreeMap<String, ThresholdSpec>,
    pub interval: Duration,
    pub retry_interval: Duration,
    pub max_attempts: u32,
    /// Passive jobs: how long a result stays valid before the job turns
    /// UNKNOWN.
    pub freshness: Option<Duration>,
}

/// A check result and the state it led to.
#[derive(Debug, Clone)]
pub struct Event {
    pub id: String,
    pub result: CheckResult,
    /// How long the check ran (zero for submitted results).
    pub duration: Duration,
    /// The state after this result.
    pub state: ServiceState,
    /// Set when the state or state type changed.
    pub change: Option<StateChange>,
}

/// Requests to a running scheduler.
#[derive(Debug)]
pub enum Control {
    /// Replace the job list.
    Reload(Vec<Job>),
    /// A result produced elsewhere, e.g. a passive check posted to the API.
    /// It is evaluated with the job's format and thresholds. The reply gets
    /// the resulting event, or `None` if there is no such job.
    Submit {
        id: String,
        output: RawOutput,
        reply: oneshot::Sender<Option<Event>>,
    },
}

struct Slot {
    job: Arc<Job>,
    state: Option<ServiceState>,
    /// When the job is due next (a run, or the freshness deadline of a
    /// passive job). Meaningless while it runs.
    due: Option<Instant>,
    running: bool,
}

struct Done {
    id: String,
    duration: Duration,
    result: CheckResult,
}

type Queue = BinaryHeap<Reverse<(Instant, String)>>;

/// Runs `jobs` until `events` is closed. `initial` holds states to continue
/// from, e.g. restored from the database, keyed by job id.
pub async fn schedule(
    jobs: Vec<Job>,
    mut initial: HashMap<String, ServiceState>,
    max_concurrent: usize,
    mut control: mpsc::Receiver<Control>,
    events: mpsc::Sender<Event>,
) {
    let permits = Arc::new(Semaphore::new(max_concurrent.max(1)));
    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<Done>();
    let mut slots: HashMap<String, Slot> = HashMap::new();
    let mut queue = Queue::new();
    let mut control_open = true;
    replace_jobs(&mut slots, &mut queue, jobs, &mut initial);
    drop(initial);

    loop {
        let next_due = queue.peek().map(|Reverse((due, _))| *due);
        let event = tokio::select! {
            () = sleep_until(next_due) => {
                let now = Instant::now();
                let mut stale = Vec::new();
                while let Some(Reverse((due, id))) = queue.peek().cloned() {
                    if due > now {
                        break;
                    }
                    queue.pop();
                    // Skip entries left behind by reloads and reschedules.
                    let Some(slot) = slots.get_mut(&id) else { continue };
                    if slot.running || slot.due != Some(due) {
                        continue;
                    }
                    let Some(command) = slot.job.command.clone() else {
                        stale.push(id);
                        continue;
                    };
                    slot.running = true;
                    let (job, permits, done) = (slot.job.clone(), permits.clone(), done_tx.clone());
                    tokio::spawn(async move {
                        let Ok(_permit) = permits.acquire_owned().await else { return };
                        let execution = run(&command, &job.options).await;
                        let result = evaluate(&execution.output, job.format, &job.thresholds);
                        let duration = execution.duration;
                        let _ = done.send(Done { id, duration, result });
                    });
                }
                for id in stale {
                    let Some(event) = expire(&mut slots, &mut queue, &id) else { continue };
                    if events.send(event).await.is_err() {
                        return;
                    }
                }
                continue;
            }
            Some(done) = done_rx.recv() => complete(&mut slots, &mut queue, done),
            msg = control.recv(), if control_open => match msg {
                Some(Control::Reload(jobs)) => {
                    replace_jobs(&mut slots, &mut queue, jobs, &mut HashMap::new());
                    None
                }
                Some(Control::Submit { id, output, reply }) => {
                    let event = submit(&mut slots, &mut queue, &id, &output);
                    let _ = reply.send(event.clone());
                    event
                }
                None => {
                    control_open = false;
                    None
                }
            },
            () = events.closed() => return,
        };
        if let Some(event) = event
            && events.send(event).await.is_err()
        {
            return;
        }
    }
}

async fn sleep_until(due: Option<Instant>) {
    match due {
        Some(due) => tokio::time::sleep_until(due).await,
        None => std::future::pending().await,
    }
}

/// Applies a result to a slot's state.
fn apply(slot: &mut Slot, id: String, result: CheckResult, duration: Duration) -> Event {
    let now = SystemTime::now();
    let max_attempts = slot.job.max_attempts;
    let (state, change) = match slot.state.as_mut() {
        Some(state) => {
            let change = state.apply(result.state, max_attempts, now);
            (state.clone(), change)
        }
        None => {
            let (state, change) = ServiceState::first(result.state, max_attempts, now);
            slot.state = Some(state.clone());
            (state, Some(change))
        }
    };
    Event {
        id,
        result,
        duration,
        state,
        change,
    }
}

fn set_due(slot: &mut Slot, queue: &mut Queue, due: Option<Instant>) {
    slot.due = due;
    if let Some(due) = due {
        queue.push(Reverse((due, slot.job.id.clone())));
    }
}

/// Records a finished run and schedules the next one.
fn complete(slots: &mut HashMap<String, Slot>, queue: &mut Queue, done: Done) -> Option<Event> {
    // The job may have been removed by a reload while it ran.
    let slot = slots.get_mut(&done.id)?;
    slot.running = false;
    let event = apply(slot, done.id, done.result, done.duration);

    let now = Instant::now();
    let due = if event.state.retrying() {
        now + slot.job.retry_interval
    } else {
        slot.due.map_or(now, |due| due + slot.job.interval).max(now)
    };
    set_due(slot, queue, Some(due));
    Some(event)
}

/// Applies a submitted result. Active jobs keep their schedule; passive
/// jobs restart their freshness deadline.
fn submit(
    slots: &mut HashMap<String, Slot>,
    queue: &mut Queue,
    id: &str,
    output: &RawOutput,
) -> Option<Event> {
    let slot = slots.get_mut(id)?;
    let result = evaluate(output, slot.job.format, &slot.job.thresholds);
    let event = apply(slot, id.to_string(), result, Duration::ZERO);
    if slot.job.command.is_none() {
        let due = slot.job.freshness.map(|f| Instant::now() + f);
        set_due(slot, queue, due);
    }
    Some(event)
}

/// A passive job got no result within its freshness: it turns UNKNOWN,
/// and stays so until a result arrives.
fn expire(slots: &mut HashMap<String, Slot>, queue: &mut Queue, id: &str) -> Option<Event> {
    let slot = slots.get_mut(id)?;
    let freshness = slot.job.freshness?;
    let output = RawOutput {
        termination: Termination::Exited(3),
        stdout: format!("UNKNOWN - no result received for {}", human(freshness)),
        stderr: String::new(),
    };
    let result = evaluate(&output, Format::Nagios, &BTreeMap::new());
    let event = apply(slot, id.to_string(), result, Duration::ZERO);
    set_due(slot, queue, Some(Instant::now() + freshness));
    Some(event)
}

/// Installs a new job list, keeping state and timing of jobs that stay.
fn replace_jobs(
    slots: &mut HashMap<String, Slot>,
    queue: &mut Queue,
    jobs: Vec<Job>,
    initial: &mut HashMap<String, ServiceState>,
) {
    let mut old = std::mem::take(slots);
    for job in jobs {
        let job = Arc::new(job);
        let slot = match old.remove(&job.id) {
            Some(mut slot) => {
                let timing_changed = slot.job.interval != job.interval
                    || slot.job.freshness != job.freshness
                    || slot.job.command.is_some() != job.command.is_some();
                slot.job = job;
                if timing_changed && !slot.running {
                    let due = first_due(&slot.job);
                    set_due(&mut slot, queue, due);
                }
                slot
            }
            None => {
                let mut slot = Slot {
                    state: initial.remove(&job.id),
                    due: None,
                    running: false,
                    job,
                };
                let due = first_due(&slot.job);
                set_due(&mut slot, queue, due);
                slot
            }
        };
        slots.insert(slot.job.id.clone(), slot);
    }
}

/// First run of an active job (spread over its interval), or the first
/// freshness deadline of a passive one.
fn first_due(job: &Job) -> Option<Instant> {
    let now = Instant::now();
    match job.command {
        Some(_) => Some(now + spread(&job.id, job.interval)),
        None => job.freshness.map(|f| now + f),
    }
}

/// A stable offset in `[0, interval)` derived from the job id.
fn spread(id: &str, interval: Duration) -> Duration {
    let mut hasher = DefaultHasher::new();
    id.hash(&mut hasher);
    let millis = interval.as_millis().max(1) as u64;
    Duration::from_millis(hasher.finish() % millis)
}

/// `26h`, `90m`, `45s`: the largest unit that divides evenly.
fn human(d: Duration) -> String {
    let secs = d.as_secs();
    match secs {
        0 => format!("{}ms", d.as_millis()),
        s if s % 86_400 == 0 => format!("{}d", s / 86_400),
        s if s % 3600 == 0 => format!("{}h", s / 3600),
        s if s % 60 == 0 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use watcher_core::{State, StateType};

    fn job(id: &str, script: &str, interval_ms: u64) -> Job {
        Job {
            id: id.into(),
            command: Some(CommandLine::Shell(script.into())),
            options: ExecOptions::default(),
            format: Format::Auto,
            thresholds: BTreeMap::new(),
            interval: Duration::from_millis(interval_ms),
            retry_interval: Duration::from_millis(interval_ms / 4),
            max_attempts: 3,
            freshness: None,
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
            HashMap::new(),
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
    async fn continues_from_initial_state() {
        let (_reload_tx, reload_rx) = mpsc::channel(1);
        let (tx, mut rx) = mpsc::channel(16);
        let (mut restored, _) = ServiceState::first(State::Critical, 3, SystemTime::now());
        restored.apply(State::Critical, 3, SystemTime::now());
        let initial = HashMap::from([("h/fail".to_string(), restored)]);
        tokio::spawn(schedule(
            vec![job("h/fail", "exit 2", 400)],
            initial,
            4,
            reload_rx,
            tx,
        ));

        // Attempt 3 of 3: the restored soft problem becomes hard.
        let event = collect(&mut rx, 1).await.remove(0);
        assert_eq!(
            (event.state.state_type, event.state.attempt),
            (StateType::Hard, 3)
        );
        assert!(event.change.unwrap().hard);
    }

    fn passive(id: &str, freshness_ms: Option<u64>) -> Job {
        Job {
            command: None,
            freshness: freshness_ms.map(Duration::from_millis),
            ..job(id, "", 60_000)
        }
    }

    async fn submit(
        control: &mpsc::Sender<Control>,
        id: &str,
        exit: i32,
        stdout: &str,
    ) -> Option<Event> {
        let (reply, rx) = oneshot::channel();
        let output = RawOutput {
            termination: Termination::Exited(exit),
            stdout: stdout.into(),
            stderr: String::new(),
        };
        control
            .send(Control::Submit {
                id: id.into(),
                output,
                reply,
            })
            .await
            .unwrap();
        rx.await.unwrap()
    }

    #[tokio::test]
    async fn passive_results_go_through_the_state_machine() {
        let (control, control_rx) = mpsc::channel(4);
        let (tx, mut rx) = mpsc::channel(16);
        tokio::spawn(schedule(
            vec![passive("h/backup", None)],
            HashMap::new(),
            4,
            control_rx,
            tx,
        ));

        let event = submit(
            &control,
            "h/backup",
            2,
            "BACKUP CRITICAL - failed | size=0B",
        )
        .await
        .unwrap();
        assert_eq!(
            (event.state.state, event.state.state_type),
            (State::Critical, StateType::Soft)
        );
        assert_eq!(event.result.message, "BACKUP CRITICAL - failed");
        // The event also goes to the event stream (storage, live updates).
        assert_eq!(collect(&mut rx, 1).await[0].id, "h/backup");

        assert!(submit(&control, "h/missing", 0, "OK").await.is_none());
        // Passive jobs never run on their own.
        assert!(
            tokio::time::timeout(Duration::from_millis(300), rx.recv())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn stale_passive_job_turns_unknown() {
        let (control, control_rx) = mpsc::channel(4);
        let (tx, mut rx) = mpsc::channel(16);
        tokio::spawn(schedule(
            vec![passive("h/cron", Some(200))],
            HashMap::new(),
            4,
            control_rx,
            tx,
        ));

        submit(&control, "h/cron", 0, "OK - ran").await.unwrap();
        let events = collect(&mut rx, 2).await;
        assert_eq!(events[0].state.state, State::Ok);
        assert_eq!(events[1].state.state, State::Unknown);
        assert_eq!(
            events[1].result.message,
            "UNKNOWN - no result received for 200ms"
        );
    }

    #[test]
    fn human_durations() {
        assert_eq!(human(Duration::from_secs(26 * 3600)), "26h");
        assert_eq!(human(Duration::from_secs(90 * 60)), "90m");
        assert_eq!(human(Duration::from_secs(86_400)), "1d");
        assert_eq!(human(Duration::from_secs(45)), "45s");
    }

    #[tokio::test]
    async fn reload_adds_jobs_and_keeps_state() {
        let (reload_tx, reload_rx) = mpsc::channel(1);
        let (tx, mut rx) = mpsc::channel(16);
        let a = job("h/a", "exit 0", 100);
        tokio::spawn(schedule(vec![a.clone()], HashMap::new(), 4, reload_rx, tx));

        let first = collect(&mut rx, 1).await;
        assert!(first[0].change.is_some());

        reload_tx
            .send(Control::Reload(vec![a, job("h/b", "exit 1", 100)]))
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

        reload_tx.send(Control::Reload(vec![])).await.unwrap();
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
