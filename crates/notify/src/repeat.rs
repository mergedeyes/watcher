//! Repeated notifications: while a problem stays open, each contact gets it
//! again every `repeat` of the rules that give it to them.
//!
//! The clock of a contact starts at the last PROBLEM they got about it, or,
//! if they got none (it started in a downtime, say), when the problem
//! started. Problems already open at startup start their clock then, so a
//! restart doesn't send a burst of repeats. Held back problems
//! (acknowledged, in a downtime, ...) keep their clock but don't repeat.
//!
//! With a notification log, [`Repeats::restore`] brings back who got which
//! problem before a restart, so the clock goes on where it was.
//!
//! Escalations (rules with `after`) use the same clock: their contacts get
//! the problem once it has been in its state for `after` without being
//! held back, then every `repeat` of the rule.

use crate::{Kind, Notification, about};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use watcher_config::{Config, Contact, state_name};
use watcher_core::State;

/// A HARD problem that is still open.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenProblem {
    /// `host/service`.
    pub id: String,
    pub state: State,
    pub attempts: u32,
    pub output: String,
    pub long_output: String,
    pub last_check: SystemTime,
    /// When it changed to this state.
    pub since: SystemTime,
    /// Held back now: acknowledged, in a downtime, muted, flapping, ...
    pub suppressed: bool,
}

/// When repeats are due, per open problem and contact.
#[derive(Default)]
pub struct Repeats {
    open: HashMap<String, Open>,
    /// The first list of open problems came in.
    started: bool,
    /// The configuration the cached `next` times come from.
    config: Option<Arc<Config>>,
    /// From the log, until the first list: per problem, who got it when.
    history: HashMap<String, Vec<(String, SystemTime)>>,
    /// When the log started; `None` without a log.
    log_start: Option<SystemTime>,
}

struct Open {
    since: SystemTime,
    /// The clock start for contacts that didn't get this problem yet.
    base: SystemTime,
    /// When each contact last got it.
    sent: HashMap<String, SystemTime>,
    /// No repeat is due before this; `None`: work it out on the next list.
    next: Option<SystemTime>,
}

/// "Never", for problems without repeating rules (until a reload).
const FAR: Duration = Duration::from_secs(10 * 365 * 86400);

impl Repeats {
    /// Starts from the notification log: `history` has the last PROBLEM
    /// each contact got per `host/service`, `log_start` is when the log
    /// began. Problems open now count from when they started (or from
    /// `log_start`, if later), not from now.
    pub fn restore(history: Vec<(String, String, SystemTime)>, log_start: SystemTime) -> Self {
        let mut map: HashMap<String, Vec<(String, SystemTime)>> = HashMap::new();
        for (id, contact, t) in history {
            map.entry(id).or_default().push((contact, t));
        }
        Repeats {
            history: map,
            log_start: Some(log_start),
            ..Repeats::default()
        }
    }

    /// A PROBLEM about `id`, in its state since `since`, went to `contacts`.
    pub fn sent(&mut self, id: &str, since: SystemTime, contacts: &[&Contact], now: SystemTime) {
        let history = &mut self.history;
        let open = self
            .open
            .entry(id.to_string())
            .or_insert_with(|| Open::new(history, id, since, since));
        open.since = since;
        open.next = None;
        for c in contacts {
            open.sent.insert(c.name.clone(), now);
        }
    }

    /// Who got a PROBLEM about `id` while it was open.
    pub fn got(&self, id: &str) -> Vec<String> {
        self.open
            .get(id)
            .map(|o| o.sent.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// `id` recovered.
    pub fn ended(&mut self, id: &str) {
        self.open.remove(id);
    }

    /// Takes the problems open now and returns the repeats due.
    pub fn due<'a>(
        &mut self,
        problems: &[OpenProblem],
        config: &'a Arc<Config>,
        public_url: Option<&str>,
        now: SystemTime,
    ) -> Vec<(Notification, Vec<&'a Contact>)> {
        // A reload may change rules and contacts.
        if !self.config.as_ref().is_some_and(|c| Arc::ptr_eq(c, config)) {
            for open in self.open.values_mut() {
                open.next = None;
            }
            self.config = Some(config.clone());
        }
        let ids: HashSet<&str> = problems.iter().map(|p| p.id.as_str()).collect();
        self.open.retain(|id, _| ids.contains(id.as_str()));
        let started = std::mem::replace(&mut self.started, true);
        let log_start = self.log_start;
        let history = &mut self.history;

        let mut out = Vec::new();
        for p in problems {
            let open = self.open.entry(p.id.clone()).or_insert_with(|| {
                // Open at startup: without a log, nothing is known about
                // it, so its clock starts now.
                let base = match started {
                    true => p.since,
                    false => log_start.map_or(now, |s| p.since.max(s)),
                };
                Open::new(history, &p.id, p.since, base)
            });
            open.since = p.since;
            if p.suppressed || open.next.is_some_and(|next| now < next) {
                continue;
            }
            let Some(service) = config.service(&p.id) else {
                continue;
            };
            let host_check = config.is_host_check(service);
            let state = state_name(p.state, host_check);
            // When each contact is due: the earliest of their rules.
            let mut at: Vec<(&Contact, SystemTime)> = Vec::new();
            let mut add = |contact: &'a Contact, t: SystemTime| match at
                .iter_mut()
                .find(|(c, _)| c.name == contact.name)
            {
                Some((_, prev)) => *prev = (*prev).min(t),
                None => at.push((contact, t)),
            };
            for (contact, every) in config.repeat_recipients(service, host_check, state) {
                let last = open.sent.get(&contact.name).copied().unwrap_or(open.base);
                add(contact, last + every);
            }
            // Escalations count from when the problem reached its state
            // (or from startup), and only take what was sent since then.
            let start = open.base.max(p.since);
            for (contact, after, every) in config.escalations(service, host_check, state) {
                let got = open
                    .sent
                    .get(&contact.name)
                    .copied()
                    .filter(|&t| t >= p.since);
                match (got, every) {
                    (None, _) => add(contact, start + after),
                    (Some(last), Some(every)) => add(contact, (last + every).max(start + after)),
                    (Some(_), None) => {}
                }
            }
            let mut due = Vec::new();
            let mut next = now + FAR;
            for (contact, t) in at {
                if t <= now {
                    open.sent.insert(contact.name.clone(), now);
                    due.push(contact);
                } else {
                    next = next.min(t);
                }
            }
            // After sending, work out the next time on the next list.
            open.next = due.is_empty().then_some(next);
            if due.is_empty() {
                continue;
            }
            let mut n = about(
                config,
                service,
                host_check,
                public_url,
                Kind::Problem,
                p.state,
            );
            n.attempts = p.attempts;
            n.output = p.output.clone();
            n.long_output = p.long_output.clone();
            n.time = p.last_check;
            n.since = Some(p.since);
            n.repeat = true;
            out.push((n, due));
        }
        self.history = HashMap::new();
        out
    }
}

impl Open {
    /// A problem that started at `since`, with what the log says about it.
    fn new(
        history: &mut HashMap<String, Vec<(String, SystemTime)>>,
        id: &str,
        since: SystemTime,
        base: SystemTime,
    ) -> Self {
        let sent = history
            .remove(id)
            .into_iter()
            .flatten()
            .filter(|&(_, t)| t >= since)
            .collect();
        Open {
            since,
            base,
            sent,
            next: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(rules: &str) -> Arc<Config> {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("watcher-repeat-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let text = format!(
            r#"
[contact.ops]
email = "ops@example.com"
[contact.boss]
email = "boss@example.com"

{rules}

[[host]]
name = "web01"
host_check = "ping"
  [host.service.ping]
  command = "true"
  [host.service.disk]
  command = "true"
"#
        );
        std::fs::write(dir.join("c.toml"), text).unwrap();
        let loaded = watcher_config::load_dir(&dir).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        Arc::new(loaded.config)
    }

    fn problem(id: &str, state: State, since: SystemTime) -> OpenProblem {
        OpenProblem {
            id: id.into(),
            state,
            attempts: 3,
            output: "DISK CRITICAL".into(),
            long_output: String::new(),
            last_check: since,
            since,
            suppressed: false,
        }
    }

    const MIN: Duration = Duration::from_secs(60);

    fn contact<'c>(config: &'c Config, name: &str) -> &'c Contact {
        config.contacts.iter().find(|c| c.name == name).unwrap()
    }

    /// Who gets a repeat at `now`.
    fn names(
        r: &mut Repeats,
        problems: &[OpenProblem],
        config: &Arc<Config>,
        now: SystemTime,
    ) -> Vec<String> {
        r.due(problems, config, None, now)
            .into_iter()
            .flat_map(|(n, c)| {
                assert!(n.repeat && n.kind == Kind::Problem);
                c.into_iter()
                    .map(move |c| format!("{} {}", n.service_id(), c.name))
            })
            .collect()
    }

    #[test]
    fn repeats_every_interval_after_the_last_notification() {
        let config = config(
            r#"
[[notify]]
contacts = ["ops"]
repeat = "30m"
"#,
        );
        let t0 = SystemTime::now();
        let p = [problem("web01/disk", State::Critical, t0)];
        let mut r = Repeats::default();
        assert!(names(&mut r, &[], &config, t0).is_empty());
        let ops = contact(&config, "ops");
        r.sent("web01/disk", t0, &[ops], t0);
        assert!(names(&mut r, &p, &config, t0 + 29 * MIN).is_empty());
        assert_eq!(
            names(&mut r, &p, &config, t0 + 30 * MIN),
            ["web01/disk ops"]
        );
        assert!(names(&mut r, &p, &config, t0 + 59 * MIN).is_empty());
        assert_eq!(
            names(&mut r, &p, &config, t0 + 60 * MIN),
            ["web01/disk ops"]
        );

        // Held back: no repeat, but the clock keeps going.
        let held = [OpenProblem {
            suppressed: true,
            ..p[0].clone()
        }];
        assert!(names(&mut r, &held, &config, t0 + 120 * MIN).is_empty());
        assert_eq!(
            names(&mut r, &p, &config, t0 + 121 * MIN),
            ["web01/disk ops"]
        );

        // Recovered: forgotten. A new problem starts its own clock.
        assert!(names(&mut r, &[], &config, t0 + 122 * MIN).is_empty());
        let again = [problem("web01/disk", State::Critical, t0 + 123 * MIN)];
        assert!(names(&mut r, &again, &config, t0 + 124 * MIN).is_empty());
        assert_eq!(
            names(&mut r, &again, &config, t0 + 153 * MIN),
            ["web01/disk ops"]
        );
    }

    #[test]
    fn startup_and_hidden_problems() {
        let config = config(
            r#"
[[notify]]
contacts = ["ops"]
repeat = "1h"
"#,
        );
        let t0 = SystemTime::now();
        let old = problem("web01/disk", State::Critical, t0 - 5 * 60 * MIN);
        let mut r = Repeats::default();
        // Open at startup: the clock starts now.
        assert!(names(&mut r, std::slice::from_ref(&old), &config, t0).is_empty());
        assert_eq!(
            names(&mut r, &[old], &config, t0 + 60 * MIN),
            ["web01/disk ops"]
        );
        // A problem that started later without a notification (say, in a
        // downtime) is due an hour after it started.
        let hidden = problem("web01/ping", State::Critical, t0);
        assert_eq!(
            names(&mut r, &[hidden], &config, t0 + 61 * MIN),
            ["web01 ops"]
        );
    }

    #[test]
    fn shortest_interval_per_contact_and_rule_filters() {
        let config = config(
            r#"
[[notify]]
contacts = ["ops"]
repeat = "4h"

[[notify]]
contacts = ["ops", "boss"]
states = ["critical"]
repeat = "1h"

[[notify]]
contacts = ["boss"]
"#,
        );
        let t0 = SystemTime::now();
        let mut r = Repeats::default();
        assert!(names(&mut r, &[], &config, t0).is_empty());
        let crit = [problem("web01/disk", State::Critical, t0)];
        assert_eq!(
            names(&mut r, &crit, &config, t0 + 60 * MIN),
            ["web01/disk ops", "web01/disk boss"]
        );
        // WARNING: only the 4h rule repeats, 4h after ops last got it.
        let warn = [problem("web01/disk", State::Warning, t0 + 61 * MIN)];
        assert!(names(&mut r, &warn, &config, t0 + 4 * 60 * MIN).is_empty());
        assert_eq!(
            names(&mut r, &warn, &config, t0 + 5 * 60 * MIN),
            ["web01/disk ops"]
        );
    }

    #[test]
    fn escalations_after_the_problem_lasts() {
        let config = config(
            r#"
[[notify]]
contacts = ["ops"]

[[notify]]
contacts = ["boss"]
after = "30m"
repeat = "1h"
"#,
        );
        let t0 = SystemTime::now();
        let mut r = Repeats::default();
        assert!(names(&mut r, &[], &config, t0).is_empty());
        let p = [problem("web01/disk", State::Critical, t0)];
        r.sent("web01/disk", t0, &[contact(&config, "ops")], t0);
        assert!(names(&mut r, &p, &config, t0 + 29 * MIN).is_empty());
        assert_eq!(
            names(&mut r, &p, &config, t0 + 30 * MIN),
            ["web01/disk boss"]
        );
        let mut got = r.got("web01/disk");
        got.sort();
        assert_eq!(got, ["boss", "ops"]);
        // Then the rule's repeat.
        assert!(names(&mut r, &p, &config, t0 + 89 * MIN).is_empty());
        assert_eq!(
            names(&mut r, &p, &config, t0 + 90 * MIN),
            ["web01/disk boss"]
        );

        // A new state starts the escalation again.
        let warn = [problem("web01/disk", State::Warning, t0 + 100 * MIN)];
        r.sent(
            "web01/disk",
            t0 + 100 * MIN,
            &[contact(&config, "ops")],
            t0 + 100 * MIN,
        );
        assert!(names(&mut r, &warn, &config, t0 + 129 * MIN).is_empty());
        assert_eq!(
            names(&mut r, &warn, &config, t0 + 130 * MIN),
            ["web01/disk boss"]
        );
    }

    #[test]
    fn held_back_problems_dont_escalate() {
        let config = config(
            r#"
[[notify]]
contacts = ["boss"]
after = "30m"
"#,
        );
        let t0 = SystemTime::now();
        let mut r = Repeats::default();
        assert!(names(&mut r, &[], &config, t0).is_empty());
        let p = problem("web01/disk", State::Critical, t0);
        let acked = OpenProblem {
            suppressed: true,
            ..p.clone()
        };
        assert!(names(&mut r, std::slice::from_ref(&acked), &config, t0 + 60 * MIN).is_empty());
        // Unacknowledged later: due right away, once (no repeat).
        let p = [p];
        assert_eq!(
            names(&mut r, &p, &config, t0 + 61 * MIN),
            ["web01/disk boss"]
        );
        assert!(names(&mut r, &p, &config, t0 + 600 * MIN).is_empty());

        // Open at startup: `after` counts from then.
        let mut r = Repeats::default();
        let old = [problem("web01/disk", State::Critical, t0 - 600 * MIN)];
        assert!(names(&mut r, &old, &config, t0).is_empty());
        assert!(names(&mut r, &old, &config, t0 + 29 * MIN).is_empty());
        assert_eq!(
            names(&mut r, &old, &config, t0 + 30 * MIN),
            ["web01/disk boss"]
        );
    }

    #[test]
    fn contacts_told_at_once_are_not_escalated_to() {
        let config = config(
            r#"
[[notify]]
contacts = ["ops"]

[[notify]]
contacts = ["ops", "boss"]
after = "30m"
"#,
        );
        let t0 = SystemTime::now();
        let mut r = Repeats::default();
        assert!(names(&mut r, &[], &config, t0).is_empty());
        let p = [problem("web01/disk", State::Critical, t0)];
        r.sent("web01/disk", t0, &[contact(&config, "ops")], t0);
        assert_eq!(
            names(&mut r, &p, &config, t0 + 30 * MIN),
            ["web01/disk boss"]
        );
    }

    #[test]
    fn restored_from_the_log_the_clock_goes_on() {
        let config = config(
            r#"
[[notify]]
contacts = ["ops"]
after = "15m"
repeat = "30m"

[[notify]]
contacts = ["boss"]
after = "90m"
"#,
        );
        let t0 = SystemTime::now();
        let entry = |c: &str, t: SystemTime| ("web01/disk".to_string(), c.to_string(), t);
        // Open for 100 minutes; ops got it at 15 and 45 minutes, boss
        // not yet (Watcher was down at 90 minutes).
        let since = t0 - 100 * MIN;
        let history = vec![
            entry("ops", since + 45 * MIN),
            // From an earlier problem: doesn't count.
            entry("boss", since - 600 * MIN),
        ];
        let mut r = Repeats::restore(history, t0 - 10_000 * MIN);
        let p = [problem("web01/disk", State::Critical, since)];
        // Boss is overdue: at once. Ops: 30 minutes after 45, at 75 → due.
        assert_eq!(
            names(&mut r, &p, &config, t0),
            ["web01/disk ops", "web01/disk boss"]
        );
        let mut got = r.got("web01/disk");
        got.sort();
        assert_eq!(got, ["boss", "ops"]);

        // Not due yet at the restart: due when it would have been.
        let mut r = Repeats::restore(
            vec![("web01/disk".into(), "ops".into(), t0 - 5 * MIN)],
            t0 - 10_000 * MIN,
        );
        let p = [problem("web01/disk", State::Critical, t0 - 20 * MIN)];
        assert!(names(&mut r, &p, &config, t0).is_empty());
        assert_eq!(
            names(&mut r, &p, &config, t0 + 25 * MIN),
            ["web01/disk ops"]
        );
        assert_eq!(r.got("web01/disk"), ["ops"]);
    }

    #[test]
    fn problems_older_than_the_log_count_from_its_start() {
        let config = config(
            r#"
[[notify]]
contacts = ["boss"]
after = "1h"
"#,
        );
        let t0 = SystemTime::now();
        let mut r = Repeats::restore(Vec::new(), t0 - 30 * MIN);
        let p = [problem("web01/disk", State::Critical, t0 - 600 * MIN)];
        assert!(names(&mut r, &p, &config, t0).is_empty());
        assert_eq!(
            names(&mut r, &p, &config, t0 + 30 * MIN),
            ["web01/disk boss"]
        );
    }

    #[test]
    fn rules_without_repeat_stay_quiet_until_a_reload() {
        let quiet = config(
            r#"
[[notify]]
contacts = ["ops"]
"#,
        );
        let t0 = SystemTime::now();
        let p = [problem("web01/disk", State::Critical, t0)];
        let mut r = Repeats::default();
        assert!(names(&mut r, &[], &quiet, t0).is_empty());
        assert!(names(&mut r, &p, &quiet, t0 + 600 * MIN).is_empty());
        let loud = config(
            r#"
[[notify]]
contacts = ["ops"]
repeat = "1h"
"#,
        );
        assert_eq!(names(&mut r, &p, &loud, t0 + 601 * MIN), ["web01/disk ops"]);
    }
}
