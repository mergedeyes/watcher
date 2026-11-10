use crate::{Delivery, Error, Kind, Notification};
use lettre::message::Mailbox;
use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use std::fmt::Write;
use std::time::{Duration, SystemTime};
use watcher_config::{Smtp, SmtpTls};

/// Sends notification emails over SMTP.
pub struct Mailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

impl Mailer {
    pub fn new(smtp: &Smtp) -> Result<Self, Error> {
        let err = |msg: String| Error::Email(msg);
        let from: Mailbox = smtp
            .from
            .parse()
            .map_err(|e| err(format!("invalid from address {:?}: {e}", smtp.from)))?;
        let builder = match smtp.tls {
            SmtpTls::Starttls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&smtp.host),
            SmtpTls::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(&smtp.host),
            SmtpTls::None => Ok(AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(
                &smtp.host,
            )),
        }
        .map_err(|e| err(format!("invalid SMTP host {:?}: {e}", smtp.host)))?;
        let mut builder = builder
            .port(smtp.port())
            .timeout(Some(Duration::from_secs(30)));

        let password = match (&smtp.password, &smtp.password_file) {
            (Some(_), Some(_)) => {
                return Err(err("set either password or password_file, not both".into()));
            }
            (Some(p), None) => Some(p.clone()),
            (None, Some(file)) => Some(
                std::fs::read_to_string(file)
                    .map_err(|e| err(format!("cannot read {}: {e}", file.display())))?
                    .trim_end_matches(['\r', '\n'])
                    .to_string(),
            ),
            (None, None) => None,
        };
        match (&smtp.username, password) {
            (Some(user), Some(pass)) => {
                builder = builder.credentials(Credentials::new(user.clone(), pass))
            }
            (None, None) => {}
            _ => return Err(err("set both username and password, or neither".into())),
        }
        Ok(Mailer {
            transport: builder.build(),
            from,
        })
    }

    pub(crate) async fn send(&self, to: &[String], n: &Notification) -> Result<(), Delivery> {
        let permanent = |msg: String| Delivery::Permanent(Error::Email(msg));
        let mut message = Message::builder().from(self.from.clone());
        for addr in to {
            let mailbox: Mailbox = addr
                .parse()
                .map_err(|e| permanent(format!("invalid address {addr:?}: {e}")))?;
            message = message.to(mailbox);
        }
        let message = message
            .subject(subject(n))
            .header(ContentType::TEXT_PLAIN)
            .body(body(n))
            .map_err(|e| permanent(e.to_string()))?;
        match self.transport.send(message).await {
            Ok(_) => Ok(()),
            Err(e) if e.is_permanent() => Err(permanent(e.to_string())),
            Err(e) => Err(Delivery::Transient(Error::Email(e.to_string()))),
        }
    }
}

/// `[Watcher] PROBLEM: web01/disk is CRITICAL`; repeats say `is still`.
pub(crate) fn subject(n: &Notification) -> String {
    let tag = match &n.instance {
        Some(name) => format!("[Watcher {name}]"),
        None => "[Watcher]".to_string(),
    };
    let is = if n.repeat { "is still" } else { "is" };
    match n.kind {
        Kind::Test => format!("{tag} TEST"),
        kind => format!(
            "{tag} {}: {} {is} {}",
            kind.as_str(),
            n.service_id(),
            n.state_name()
        ),
    }
}

/// The first line of a message: `web01/disk is CRITICAL (was WARNING)`.
pub(crate) fn summary(n: &Notification) -> String {
    let id = n.service_id();
    let state = n.state_name();
    match (n.kind, n.previous_state_name()) {
        (Kind::Test, _) => "TEST".to_string(),
        (Kind::Problem, _) if n.repeat => match n.since {
            Some(since) => format!("{id} is still {state} ({})", open_for(since, n.time)),
            None => format!("{id} is still {state}"),
        },
        (Kind::FlappingStart, _) => {
            format!("{id} is flapping ({state}); notifications are held back until it settles")
        }
        (Kind::FlappingStop, _) => format!("{id} stopped flapping ({state})"),
        (Kind::DowntimeStart, _) => {
            format!("{id} is in a downtime ({state}); no notifications until it ends")
        }
        (Kind::DowntimeEnd, _) => format!("{id}: downtime ended ({state})"),
        (_, Some(prev)) => format!("{id} is {state} (was {prev})"),
        (_, None) => format!("{id} is {state}"),
    }
}

/// The details under the output: host, service, state, times and the
/// link, as `(label, value)`.
pub(crate) fn facts(n: &Notification) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    let host = if n.address == n.host {
        n.host.clone()
    } else {
        format!("{} ({})", n.host, n.address)
    };
    out.push(("Host", host));
    match (n.host_check, n.service.is_empty()) {
        (true, true) => {}
        (true, false) => out.push(("Check", format!("{} (host check, {})", n.service, n.state))),
        (false, _) => out.push(("Service", n.service.clone())),
    }
    let state = n.state_name();
    let state = if n.kind == Kind::Problem && n.attempts > 1 && !n.repeat {
        format!("{state}, confirmed after {} attempts", n.attempts)
    } else {
        state.to_string()
    };
    out.push(("State", state));
    if let Some(since) = n.since.filter(|_| n.repeat) {
        out.push((
            "Since",
            humantime::format_rfc3339_seconds(since).to_string(),
        ));
    }
    out.push((
        "Time",
        humantime::format_rfc3339_seconds(n.time).to_string(),
    ));
    if let Some(url) = &n.url {
        out.push(("Details", url.clone()));
    }
    out
}

pub(crate) fn body(n: &Notification) -> String {
    let mut b = summary(n);
    b.push_str("\n\n");
    b.push_str(&n.output);
    b.push('\n');
    if !n.long_output.is_empty() {
        b.push('\n');
        b.push_str(&n.long_output);
        b.push('\n');
    }
    b.push('\n');
    for (label, value) in facts(n) {
        let _ = writeln!(b, "{:<10}{value}", format!("{label}:"));
    }
    b
}

/// `for 2h 10m`: whole minutes from `since` to `now`.
fn open_for(since: SystemTime, now: SystemTime) -> String {
    let secs = now.duration_since(since).unwrap_or_default().as_secs();
    let minutes = Duration::from_secs((secs / 60).max(1) * 60);
    format!("for {}", humantime::format_duration(minutes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use watcher_core::State;

    fn notification() -> Notification {
        Notification {
            kind: Kind::Problem,
            host: "web01".into(),
            address: "10.0.0.5".into(),
            service: "disk".into(),
            host_check: false,
            state: State::Critical,
            previous_state: Some(State::Ok),
            attempts: 3,
            output: "DISK CRITICAL - free space: / 120 MB (2%)".into(),
            long_output: "/boot 68 MB (69%)".into(),
            time: SystemTime::UNIX_EPOCH,
            since: Some(SystemTime::UNIX_EPOCH),
            repeat: false,
            url: Some("https://w.example/services/web01/disk".into()),
            instance: None,
        }
    }

    #[test]
    fn formats_messages() {
        let n = notification();
        assert_eq!(subject(&n), "[Watcher] PROBLEM: web01/disk is CRITICAL");
        let named = Notification {
            instance: Some("msp-hub".into()),
            ..n.clone()
        };
        assert_eq!(
            subject(&named),
            "[Watcher msp-hub] PROBLEM: web01/disk is CRITICAL"
        );
        assert_eq!(
            body(&n),
            "web01/disk is CRITICAL (was OK)\n\n\
             DISK CRITICAL - free space: / 120 MB (2%)\n\n\
             /boot 68 MB (69%)\n\n\
             Host:     web01 (10.0.0.5)\n\
             Service:  disk\n\
             State:    CRITICAL, confirmed after 3 attempts\n\
             Time:     1970-01-01T00:00:00Z\n\
             Details:  https://w.example/services/web01/disk\n"
        );
    }

    #[test]
    fn formats_repeats() {
        let start = SystemTime::UNIX_EPOCH;
        let n = Notification {
            repeat: true,
            previous_state: None,
            long_output: String::new(),
            since: Some(start),
            time: start + Duration::from_secs(2 * 3600 + 10 * 60 + 42),
            ..notification()
        };
        assert_eq!(
            subject(&n),
            "[Watcher] PROBLEM: web01/disk is still CRITICAL"
        );
        assert_eq!(
            body(&n),
            "web01/disk is still CRITICAL (for 2h 10m)\n\n\
             DISK CRITICAL - free space: / 120 MB (2%)\n\n\
             Host:     web01 (10.0.0.5)\n\
             Service:  disk\n\
             State:    CRITICAL\n\
             Since:    1970-01-01T00:00:00Z\n\
             Time:     1970-01-01T02:10:42Z\n\
             Details:  https://w.example/services/web01/disk\n"
        );
    }

    #[test]
    fn validates_settings() {
        let smtp = |from: &str| Smtp {
            host: "localhost".into(),
            port: None,
            tls: SmtpTls::None,
            username: Some("u".into()),
            password: None,
            password_file: None,
            from: from.into(),
        };
        assert!(Mailer::new(&smtp("not an address")).is_err());
        // Username without password.
        assert!(Mailer::new(&smtp("w@example.com")).is_err());
    }
}
