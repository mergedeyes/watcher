//! Notifications on hard state changes: email and webhooks.
//!
//! A problem is a hard change to a non-OK state; a recovery is a hard
//! change back to OK from a problem. Soft changes, the first OK result of a
//! service and restored states never notify. A host check notifies as the
//! host: DOWN and UP, not OK to WARNING. While a service flaps, only the
//! start and end of flapping notify.

mod email;
mod webhook;

use serde::Serialize;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::{mpsc, watch};
use tracing::{info, warn};
use watcher_config::{Config, Contact, Smtp};
use watcher_core::{HostState, State, StateType};
use watcher_exec::scheduler::Event;

pub use email::Mailer;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("email: {0}")]
    Email(String),
    #[error("webhook: {0}")]
    Webhook(String),
    #[error("contact has an email address, but no [smtp] server is configured")]
    NoSmtp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Problem,
    Recovery,
    /// The state started changing too often; problems are held back.
    FlappingStart,
    FlappingStop,
    /// Sent by `watcher notify test`.
    Test,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Problem => "PROBLEM",
            Kind::Recovery => "RECOVERY",
            Kind::FlappingStart => "FLAPPINGSTART",
            Kind::FlappingStop => "FLAPPINGSTOP",
            Kind::Test => "TEST",
        }
    }
}

/// What a notification says; also the JSON body of webhooks.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Notification {
    pub kind: Kind,
    pub host: String,
    pub address: String,
    pub service: String,
    /// About the host: `service` is its host check, and the host state
    /// follows from `state`.
    pub host_check: bool,
    pub state: State,
    /// The state before, when it differs (not set when a soft problem was
    /// just confirmed).
    pub previous_state: Option<State>,
    /// Attempts it took to confirm the problem.
    pub attempts: u32,
    pub output: String,
    pub long_output: String,
    #[serde(serialize_with = "rfc3339")]
    pub time: SystemTime,
    /// Link to the service, if `public_url` is set.
    pub url: Option<String>,
}

impl Notification {
    /// The message sent by `watcher notify test` and `watcher init`.
    pub fn test(contact: &str, public_url: Option<String>) -> Self {
        Notification {
            kind: Kind::Test,
            host: "watcher".into(),
            address: "watcher".into(),
            service: "test".into(),
            host_check: false,
            state: State::Ok,
            previous_state: None,
            attempts: 1,
            output: format!("Test notification for contact {contact}."),
            long_output: String::new(),
            time: SystemTime::now(),
            url: public_url,
        }
    }

    /// `web01/disk`, or `web01` for a host notification.
    pub fn service_id(&self) -> String {
        match self.host_check {
            true => self.host.clone(),
            false => format!("{}/{}", self.host, self.service),
        }
    }

    /// `CRITICAL`, or `DOWN` for a host notification.
    pub fn state_name(&self) -> &'static str {
        self.name(self.state)
    }

    pub fn previous_state_name(&self) -> Option<&'static str> {
        self.previous_state.map(|s| self.name(s))
    }

    fn name(&self, state: State) -> &'static str {
        match self.host_check {
            true => HostState::from_check(state).as_str(),
            false => state.as_str(),
        }
    }
}

fn rfc3339<S: serde::Serializer>(t: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
    s.collect_str(&humantime::format_rfc3339_seconds(*t))
}

/// Decides whether `event` notifies, and whom.
pub fn decide<'a>(
    event: &Event,
    config: &'a Config,
    public_url: Option<&str>,
) -> Option<(Notification, Vec<&'a Contact>)> {
    let service = config.services.iter().find(|s| s.id == event.id)?;
    let host_check = config.is_host_check(service);
    let to = event.state.state;
    // The hard state before: soft problems always start from a hard OK.
    let before = event.change.and_then(|c| c.from).map(|(state, t)| {
        if t == StateType::Soft {
            State::Ok
        } else {
            state
        }
    });
    let problem = |s: State| match host_check {
        true => HostState::from_check(s) == HostState::Down,
        false => s != State::Ok,
    };
    let kind = match event.flap {
        Some(true) => Kind::FlappingStart,
        Some(false) => Kind::FlappingStop,
        // Flapping holds back problems and recoveries.
        None if event.state.flapping => return None,
        None => {
            event.change.filter(|c| c.hard)?;
            match (problem(to), before.map(problem)) {
                (false, Some(true)) => Kind::Recovery,
                (false, _) => return None,
                // A host stays DOWN from CRITICAL to UNKNOWN.
                (true, Some(true)) if host_check => return None,
                (true, _) => Kind::Problem,
            }
        }
    };
    let previous = match (event.flap, host_check) {
        (Some(_), _) => None,
        (None, true) => before,
        // A just confirmed problem has no other state to name.
        (None, false) => event.change.and_then(|c| c.from).map(|(s, _)| s),
    };

    let check = config.check(service);
    let host = config.host(service);
    let contacts: Vec<&Contact> = check
        .notify
        .iter()
        .filter_map(|name| config.contacts.iter().find(|c| c.name == *name))
        .collect();
    if contacts.is_empty() {
        return None;
    }

    let url = public_url.map(|base| {
        format!(
            "{}/services/{}/{}",
            base.trim_end_matches('/'),
            url_segment(&host.name),
            url_segment(&check.name)
        )
    });
    let notification = Notification {
        kind,
        host: host.name.clone(),
        address: host.address.clone(),
        service: check.name.clone(),
        host_check,
        state: to,
        previous_state: previous.filter(|p| *p != to),
        attempts: match event.state.state_type {
            StateType::Hard => event.state.attempt,
            StateType::Soft => 1,
        },
        output: event.result.message.clone(),
        long_output: event.result.long_output.clone(),
        time: event.state.last_check,
        url,
    };
    Some((notification, contacts))
}

/// Percent-encodes everything but unreserved characters.
fn url_segment(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Delivers notifications over the configured channels.
pub struct Notifier {
    mailer: Option<Mailer>,
    http: reqwest::Client,
    public_url: Option<String>,
}

impl Notifier {
    pub fn new(smtp: Option<&Smtp>, public_url: Option<String>) -> Result<Self, Error> {
        // reqwest needs a process-wide rustls provider; use ring, as the
        // rest of Watcher does. Fails harmlessly if one is installed.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mailer = smtp.map(Mailer::new).transpose()?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent(concat!("watcher/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| Error::Webhook(e.to_string()))?;
        Ok(Notifier {
            mailer,
            http,
            public_url,
        })
    }

    pub fn public_url(&self) -> Option<&str> {
        self.public_url.as_deref()
    }

    /// Sends `n` to every channel of `contact`, retrying transient
    /// failures. Returns one result per channel.
    pub async fn deliver(
        &self,
        contact: &Contact,
        n: &Notification,
    ) -> Vec<(&'static str, Result<(), Error>)> {
        let mut results = Vec::new();
        if !contact.email.is_empty() {
            let result = match &self.mailer {
                Some(mailer) => retry(|| mailer.send(&contact.email, n)).await,
                None => Err(Error::NoSmtp),
            };
            results.push(("email", result));
        }
        if let Some(url) = &contact.webhook {
            results.push(("webhook", retry(|| webhook::post(&self.http, url, n)).await));
        }
        results
    }
}

/// Retries up to three times on transient errors, waiting 1 s, then 5 s.
async fn retry<F, Fut>(mut attempt: F) -> Result<(), Error>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<(), Delivery>>,
{
    let mut delays = [Duration::from_secs(1), Duration::from_secs(5)].into_iter();
    loop {
        match attempt().await {
            Ok(()) => return Ok(()),
            Err(Delivery::Transient(err)) => match delays.next() {
                Some(delay) => tokio::time::sleep(delay).await,
                None => return Err(err),
            },
            Err(Delivery::Permanent(err)) => return Err(err),
        }
    }
}

/// A failed delivery attempt.
enum Delivery {
    /// Worth retrying: network problems, 5xx, 429, temporary SMTP errors.
    Transient(Error),
    Permanent(Error),
}

/// Problems that would make notifications fail, for warnings at startup.
pub fn setup_warnings(config: &Config, notifier: &Notifier) -> Vec<String> {
    let mut warnings = Vec::new();
    if notifier.mailer.is_none() {
        for c in config.contacts.iter().filter(|c| !c.email.is_empty()) {
            warnings.push(format!(
                "contact {:?} has an email address, but no [smtp] server is configured in watcher.toml",
                c.name
            ));
        }
    }
    warnings
}

/// Sends notifications for `events` until the channel closes. `config`
/// follows reloads.
pub async fn run(
    notifier: Arc<Notifier>,
    config: watch::Receiver<Arc<Config>>,
    mut events: mpsc::Receiver<Event>,
) {
    while let Some(event) = events.recv().await {
        let config = config.borrow().clone();
        let Some((n, contacts)) = decide(&event, &config, notifier.public_url()) else {
            continue;
        };
        for contact in contacts {
            let (notifier, contact, n) = (notifier.clone(), contact.clone(), n.clone());
            // Deliveries retry with delays; don't hold up later events.
            tokio::spawn(async move {
                for (channel, result) in notifier.deliver(&contact, &n).await {
                    match result {
                        Ok(()) => info!(
                            "notified {} by {channel}: {} {} is {}",
                            contact.name,
                            n.kind.as_str(),
                            n.service_id(),
                            n.state_name()
                        ),
                        Err(err) => warn!(
                            "notifying {} about {} failed: {err}",
                            contact.name,
                            n.service_id()
                        ),
                    }
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use watcher_core::{Format, RawOutput, ServiceState, Termination, evaluate};

    fn config() -> Config {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("watcher-notify-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("c.toml"),
            r#"
[contact.ops]
email = "ops@example.com"

[[host]]
name = "web 01"
address = "10.0.0.5"

[[check]]
name = "disk"
hosts = ["web 01"]
command = "true"
notify = ["ops"]

[[check]]
name = "quiet"
hosts = ["web 01"]
command = "true"

[[host]]
name = "router"
check = "ping"

[[check]]
name = "ping"
hosts = ["router"]
command = "true"
notify = ["ops"]
"#,
        )
        .unwrap();
        let config = watcher_config::load_dir(&dir).unwrap().config;
        std::fs::remove_dir_all(&dir).unwrap();
        config
    }

    /// Runs results through the state machine (max 3 attempts, flap
    /// detection on) and returns the events.
    fn events(id: &str, results: &[(i32, &str)]) -> Vec<Event> {
        let mut state: Option<ServiceState> = None;
        results
            .iter()
            .map(|(code, out)| {
                let raw = RawOutput {
                    termination: Termination::Exited(*code),
                    stdout: (*out).into(),
                    stderr: String::new(),
                };
                let result = evaluate(&raw, Format::Auto, &BTreeMap::new());
                let now = SystemTime::now();
                let change = match &mut state {
                    Some(s) => s.apply(result.state, 3, now),
                    None => {
                        let (s, c) = ServiceState::first(result.state, 3, now);
                        state = Some(s);
                        Some(c)
                    }
                };
                let s = state.as_mut().unwrap();
                let flap = s.detect_flapping(Some(watcher_core::FlapThresholds::DEFAULT));
                Event {
                    id: id.into(),
                    result,
                    duration: Duration::ZERO,
                    state: s.clone(),
                    change,
                    flap,
                }
            })
            .collect()
    }

    #[test]
    fn notifies_on_hard_problems_and_recoveries_only() {
        let config = config();
        let evs = events(
            "web 01/disk",
            &[
                (0, "OK"),
                (2, "DISK CRITICAL"),
                (2, "DISK CRITICAL"),
                (2, "DISK CRITICAL - full"),
                (1, "DISK WARNING"),
                (0, "DISK OK"),
            ],
        );
        let decided: Vec<_> = evs
            .iter()
            .map(|e| decide(e, &config, Some("https://w.example/")).map(|(n, c)| (n, c.len())))
            .collect();
        // First OK, then two soft failures: nothing.
        assert!(decided[..3].iter().all(Option::is_none));

        let (problem, contacts) = decided[3].clone().unwrap();
        assert_eq!(contacts, 1);
        assert_eq!(
            (
                problem.kind,
                problem.state,
                problem.previous_state,
                problem.attempts
            ),
            (Kind::Problem, State::Critical, None, 3)
        );
        assert_eq!(problem.output, "DISK CRITICAL - full");
        assert_eq!(problem.address, "10.0.0.5");
        assert_eq!(
            problem.url.as_deref(),
            Some("https://w.example/services/web%2001/disk")
        );

        // Hard CRITICAL -> WARNING is a hard change: a problem again.
        let (change, _) = decided[4].clone().unwrap();
        assert_eq!(
            (change.kind, change.state, change.previous_state),
            (Kind::Problem, State::Warning, Some(State::Critical))
        );

        let (recovery, _) = decided[5].clone().unwrap();
        assert_eq!(
            (recovery.kind, recovery.state, recovery.previous_state),
            (Kind::Recovery, State::Ok, Some(State::Warning))
        );
    }

    #[test]
    fn soft_recovery_and_no_contacts_stay_quiet() {
        let config = config();
        let evs = events("web 01/disk", &[(0, "OK"), (2, "CRIT"), (0, "OK")]);
        assert!(evs.iter().all(|e| decide(e, &config, None).is_none()));
        let evs = events("web 01/quiet", &[(2, "CRIT"), (2, "CRIT"), (2, "CRIT")]);
        assert!(evs[2].change.unwrap().hard);
        assert!(decide(&evs[2], &config, None).is_none());
    }

    #[test]
    fn host_checks_notify_as_the_host() {
        let config = config();
        let sent = |results: &[(i32, &str)]| -> Vec<_> {
            events("router/ping", results)
                .iter()
                .filter_map(|e| decide(e, &config, None))
                .map(|(n, _)| {
                    (
                        n.kind,
                        n.service_id(),
                        n.state_name(),
                        n.previous_state_name(),
                    )
                })
                .collect()
        };
        // WARNING is still UP.
        let warning = [
            (0, "PING OK"),
            (1, "PING WARNING"),
            (1, "W"),
            (1, "W"),
            (0, "OK"),
        ];
        assert_eq!(sent(&warning), []);
        // CRITICAL to UNKNOWN is still DOWN.
        let down = [
            (0, "PING OK"),
            (2, "PING CRITICAL"),
            (2, "C"),
            (2, "C"),
            (3, "U"),
            (0, "OK"),
        ];
        assert_eq!(
            sent(&down),
            [
                (Kind::Problem, "router".into(), "DOWN", Some("UP")),
                (Kind::Recovery, "router".into(), "UP", Some("DOWN")),
            ]
        );
    }

    #[test]
    fn flapping_holds_back_problems() {
        let config = config();
        let mut results = vec![(0, "OK")];
        for _ in 0..6 {
            results.extend([(2, "CRIT"), (2, "CRIT"), (2, "CRIT"), (0, "OK")]);
        }
        results.extend([(0, "OK"); 21]);
        let evs = events("web 01/disk", &results);
        let kinds: Vec<Kind> = evs
            .iter()
            .filter_map(|e| decide(e, &config, None))
            .map(|(n, _)| n.kind)
            .collect();
        // Hard problems and recoveries until flapping starts, then nothing
        // until it ends.
        let start = kinds
            .iter()
            .position(|k| *k == Kind::FlappingStart)
            .unwrap();
        assert!(
            kinds[..start]
                .iter()
                .all(|k| matches!(k, Kind::Problem | Kind::Recovery))
        );
        assert_eq!(&kinds[start + 1..], [Kind::FlappingStop]);
    }

    #[test]
    fn webhook_json() {
        let config = config();
        let evs = events(
            "web 01/disk",
            &[(2, "CRIT | used=95%"), (2, "CRIT"), (2, "CRIT")],
        );
        let (n, _) = decide(&evs[2], &config, None).unwrap();
        let json = serde_json::to_value(&n).unwrap();
        assert_eq!(json["kind"], "problem");
        assert_eq!(json["state"], "critical");
        assert_eq!(json["previous_state"], serde_json::Value::Null);
        assert_eq!(json["host"], "web 01");
        assert!(json["time"].as_str().unwrap().ends_with('Z'));
    }

    #[test]
    fn warns_about_email_without_smtp() {
        let config = config();
        let notifier = Notifier::new(None, None).unwrap();
        assert_eq!(setup_warnings(&config, &notifier).len(), 1);
    }
}
