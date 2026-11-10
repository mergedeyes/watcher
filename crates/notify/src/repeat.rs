//! Repeated notifications: while a problem stays open, each contact gets it
//! again every `repeat` of the rules that give it to them.
//!
//! The clock of a contact starts at the last PROBLEM they got about it, or,
//! if they got none (it started in a downtime, say), when the problem
//! started. Problems already open at startup start their clock then, so a
//! restart doesn't send a burst of repeats. Held back problems
//! (acknowledged, in a downtime, ...) keep their clock but don't repeat.

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
    /// A PROBLEM about `id`, in its state since `since`, went to `contacts`.
    pub fn sent(&mut self, id: &str, since: SystemTime, contacts: &[&Contact], now: SystemTime) {
        let open = self.open.entry(id.to_string()).or_insert_with(|| Open {
            since,
            base: since,
            sent: HashMap::new(),
            next: None,
        });
        open.since = since;
        open.next = None;
        for c in contacts {
            open.sent.insert(c.name.clone(), now);
        }
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

        let mut out = Vec::new();
        for p in problems {
            let open = self.open.entry(p.id.clone()).or_insert_with(|| Open {
                since: p.since,
                base: if started { p.since } else { now },
                sent: HashMap::new(),
                next: None,
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
            let mut due = Vec::new();
            let mut next = now + FAR;
            for (contact, every) in config.repeat_recipients(service, host_check, state) {
                let last = open.sent.get(&contact.name).copied().unwrap_or(open.base);
                let at = match last + every <= now {
                    true => {
                        open.sent.insert(contact.name.clone(), now);
                        due.push(contact);
                        now + every
                    }
                    false => last + every,
                };
                next = next.min(at);
            }
            open.next = Some(next);
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
        out
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
