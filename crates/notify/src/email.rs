use crate::{Delivery, Error, Kind, Notification};
use lettre::message::Mailbox;
use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use std::fmt::Write;
use std::time::Duration;
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

/// `[Watcher] PROBLEM: web01/disk is CRITICAL`
pub(crate) fn subject(n: &Notification) -> String {
    match n.kind {
        Kind::Test => "[Watcher] TEST".to_string(),
        kind => format!(
            "[Watcher] {}: {} is {}",
            kind.as_str(),
            n.service_id(),
            n.state_name()
        ),
    }
}

pub(crate) fn body(n: &Notification) -> String {
    let mut b = String::new();
    let id = n.service_id();
    let state = n.state_name();
    let _ = match (n.kind, n.previous_state_name()) {
        (Kind::Test, _) => writeln!(b, "TEST"),
        (Kind::FlappingStart, _) => writeln!(
            b,
            "{id} is flapping ({state}); notifications are held back until it settles"
        ),
        (Kind::FlappingStop, _) => writeln!(b, "{id} stopped flapping ({state})"),
        (_, Some(prev)) => writeln!(b, "{id} is {state} (was {prev})"),
        (_, None) => writeln!(b, "{id} is {state}"),
    };
    b.push('\n');
    b.push_str(&n.output);
    b.push('\n');
    if !n.long_output.is_empty() {
        b.push('\n');
        b.push_str(&n.long_output);
        b.push('\n');
    }
    b.push('\n');
    let host = if n.address == n.host {
        n.host.clone()
    } else {
        format!("{} ({})", n.host, n.address)
    };
    let state = if n.kind == Kind::Problem && n.attempts > 1 {
        format!("{state}, confirmed after {} attempts", n.attempts)
    } else {
        state.to_string()
    };
    let _ = writeln!(b, "Host:     {host}");
    let _ = match n.host_check {
        true => writeln!(b, "Check:    {} (host check, {})", n.service, n.state),
        false => writeln!(b, "Service:  {}", n.service),
    };
    let _ = writeln!(b, "State:    {state}");
    let _ = writeln!(b, "Time:     {}", humantime::format_rfc3339_seconds(n.time));
    if let Some(url) = &n.url {
        let _ = writeln!(b, "Details:  {url}");
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;
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
            url: Some("https://w.example/services/web01/disk".into()),
        }
    }

    #[test]
    fn formats_messages() {
        let n = notification();
        assert_eq!(subject(&n), "[Watcher] PROBLEM: web01/disk is CRITICAL");
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
