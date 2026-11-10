//! Chat, push and paging services: Slack, Microsoft Teams, Telegram, ntfy,
//! Gotify and PagerDuty, and scripts. Each gets the same text as an email
//! (summary, output, facts), in the form the service shows best.

use crate::email::{facts, subject, summary};
use crate::webhook::reason;
use crate::{Delivery, Error, Kind, Notification};
use serde_json::{Value, json};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use watcher_config::{Gotify, Ntfy, Telegram};
use watcher_exec::CommandLine;

/// Fixed API addresses of hosted services; tests point them elsewhere.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub telegram: String,
    pub pagerduty: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Endpoints {
            telegram: "https://api.telegram.org".into(),
            pagerduty: "https://events.pagerduty.com/v2/enqueue".into(),
        }
    }
}

/// How bad a notification is, for colours and priorities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    Critical,
    Warning,
    Unknown,
    Ok,
    Info,
}

fn level(n: &Notification) -> Level {
    match n.kind {
        Kind::Problem => match n.state_name() {
            "CRITICAL" | "DOWN" => Level::Critical,
            "WARNING" => Level::Warning,
            _ => Level::Unknown,
        },
        Kind::Recovery => Level::Ok,
        _ => Level::Info,
    }
}

/// Output cut to `max` characters, for services with message limits.
fn cut(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &text[..i]),
        None => text.to_string(),
    }
}

/// The output and long output, as one text.
fn output(n: &Notification, max: usize) -> String {
    let text = match n.long_output.is_empty() {
        true => n.output.clone(),
        false => format!("{}\n{}", n.output, n.long_output),
    };
    cut(&text, max)
}

/// Sends a request; 2xx is success, 429 and 5xx are worth retrying. The
/// service's answer is kept in the error, since it usually says what is
/// wrong ("chat not found"); URLs are left out, as some hold tokens.
async fn send(req: reqwest::RequestBuilder, channel: &'static str) -> Result<(), Delivery> {
    let err = |msg: String| Error::Channel(channel, msg);
    let resp = req
        .send()
        .await
        .map_err(|e| Delivery::Transient(err(reason(&e.without_url()))))?;
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let answer = resp.text().await.unwrap_or_default();
    let answer = cut(answer.trim(), 200);
    let msg = match answer.is_empty() {
        true => format!("answered {status}"),
        false => format!("answered {status}: {answer}"),
    };
    if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        Err(Delivery::Transient(err(msg)))
    } else {
        Err(Delivery::Permanent(err(msg)))
    }
}

/// `&`, `<` and `>` as entities, for Slack's mrkdwn and Telegram's HTML.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub(crate) fn slack_message(n: &Notification) -> Value {
    let color = match level(n) {
        Level::Critical => "#e5484d",
        Level::Warning => "#f5a524",
        Level::Unknown => "#8e4ec6",
        Level::Ok => "#2da44e",
        Level::Info => "#8b949e",
    };
    let mut text = format!("*{}*\n{}", escape(&summary(n)), escape(&output(n, 2500)));
    for (label, value) in facts(n) {
        text += &format!("\n{label}: {}", escape(&value));
    }
    json!({
        "text": subject(n),
        "attachments": [{ "color": color, "text": text }],
    })
}

pub(crate) async fn slack(
    http: &reqwest::Client,
    url: &str,
    n: &Notification,
) -> Result<(), Delivery> {
    send(http.post(url).json(&slack_message(n)), "slack").await
}

/// An Adaptive Card, which Teams Workflows webhooks and the old
/// connectors both show.
pub(crate) fn teams_message(n: &Notification) -> Value {
    let color = match level(n) {
        Level::Critical => "Attention",
        Level::Warning | Level::Unknown => "Warning",
        Level::Ok => "Good",
        Level::Info => "Default",
    };
    let all = facts(n);
    let url = all.iter().find(|(l, _)| *l == "Details").map(|(_, v)| v);
    let facts: Vec<Value> = all
        .iter()
        .filter(|(label, _)| *label != "Details")
        .map(|(label, value)| json!({ "title": label, "value": value }))
        .collect();
    let mut card = json!({
        "$schema": "http://adaptivecards.io/schemas/adaptive-card.json",
        "type": "AdaptiveCard",
        "version": "1.4",
        "body": [
            { "type": "TextBlock", "text": summary(n), "weight": "Bolder", "size": "Medium",
              "color": color, "wrap": true },
            { "type": "TextBlock", "text": output(n, 4000), "wrap": true, "fontType": "Monospace" },
            { "type": "FactSet", "facts": facts },
        ],
    });
    if let Some(url) = url {
        card["actions"] = json!([{ "type": "Action.OpenUrl", "title": "Details", "url": url }]);
    }
    json!({
        "type": "message",
        "attachments": [{
            "contentType": "application/vnd.microsoft.card.adaptive",
            "contentUrl": null,
            "content": card,
        }],
    })
}

pub(crate) async fn teams(
    http: &reqwest::Client,
    url: &str,
    n: &Notification,
) -> Result<(), Delivery> {
    send(http.post(url).json(&teams_message(n)), "teams").await
}

pub(crate) fn telegram_message(chat: &str, n: &Notification) -> Value {
    // Messages are limited to 4096 characters.
    let mut text = format!(
        "<b>{}</b>\n\n<pre>{}</pre>\n",
        escape(&summary(n)),
        escape(&output(n, 3000))
    );
    for (label, value) in facts(n) {
        text += &match label {
            "Details" => format!("\n<a href=\"{}\">Details</a>", escape(&value)),
            _ => format!("\n{label}: {}", escape(&value)),
        };
    }
    json!({
        "chat_id": chat,
        "text": text,
        "parse_mode": "HTML",
        "link_preview_options": { "is_disabled": true },
    })
}

pub(crate) async fn telegram(
    http: &reqwest::Client,
    api: &str,
    t: &Telegram,
    n: &Notification,
) -> Result<(), Delivery> {
    let url = format!("{}/bot{}/sendMessage", api.trim_end_matches('/'), t.token);
    send(
        http.post(url).json(&telegram_message(&t.chat, n)),
        "telegram",
    )
    .await
}

/// ntfy's JSON form (posted to the server, with the topic inside), since
/// headers can't carry every title.
pub(crate) fn ntfy_message(topic: &str, n: &Notification) -> Value {
    let (priority, tag) = match level(n) {
        Level::Critical => (5, Some("rotating_light")),
        Level::Warning => (4, Some("warning")),
        Level::Unknown => (4, Some("grey_question")),
        Level::Ok => (3, Some("white_check_mark")),
        Level::Info => (3, None),
    };
    let mut message = output(n, 3000);
    let mut click = None;
    for (label, value) in facts(n) {
        match label {
            "Details" => click = Some(value),
            _ => message += &format!("\n{label}: {value}"),
        }
    }
    let mut out = json!({
        "topic": topic,
        "title": summary(n),
        "message": message,
        "priority": priority,
        "tags": tag.into_iter().collect::<Vec<_>>(),
    });
    if let Some(url) = click {
        out["click"] = json!(url);
    }
    out
}

pub(crate) async fn ntfy(
    http: &reqwest::Client,
    target: &Ntfy,
    n: &Notification,
) -> Result<(), Delivery> {
    let url = target.url.trim_end_matches('/');
    let Some((server, topic)) = url.rsplit_once('/').filter(|(s, _)| s.contains("://")) else {
        return Err(Delivery::Permanent(Error::Channel(
            "ntfy",
            "expected a topic URL like https://ntfy.sh/mytopic".into(),
        )));
    };
    let mut req = http.post(server).json(&ntfy_message(topic, n));
    if let Some(token) = &target.token {
        req = req.bearer_auth(token);
    }
    send(req, "ntfy").await
}

pub(crate) fn gotify_message(n: &Notification) -> Value {
    let priority = match level(n) {
        Level::Critical => 8,
        Level::Warning | Level::Unknown => 5,
        Level::Ok => 3,
        Level::Info => 2,
    };
    let mut message = output(n, 4000);
    let mut click = None;
    for (label, value) in facts(n) {
        match label {
            "Details" => click = Some(value),
            _ => message += &format!("\n{label}: {value}"),
        }
    }
    let mut out = json!({ "title": summary(n), "message": message, "priority": priority });
    if let Some(url) = click {
        out["extras"] = json!({ "client::notification": { "click": { "url": url } } });
    }
    out
}

pub(crate) async fn gotify(
    http: &reqwest::Client,
    target: &Gotify,
    n: &Notification,
) -> Result<(), Delivery> {
    let url = format!("{}/message", target.url.trim_end_matches('/'));
    let req = http
        .post(url)
        .header("X-Gotify-Key", &target.token)
        .json(&gotify_message(n));
    send(req, "gotify").await
}

/// PagerDuty events: a problem triggers an incident, its recovery resolves
/// it (both by the same key, one per host or service). Other kinds don't
/// go to PagerDuty: `None`.
pub(crate) fn pagerduty_events(key: &str, n: &Notification) -> Option<Vec<Value>> {
    let dedup = match &n.instance {
        Some(name) => format!("watcher/{name}/{}", n.service_id()),
        None => format!("watcher/{}", n.service_id()),
    };
    let trigger = |dedup: &str| {
        let severity = match level(n) {
            Level::Critical => "critical",
            Level::Warning => "warning",
            Level::Unknown => "error",
            Level::Ok | Level::Info => "info",
        };
        let details: serde_json::Map<String, Value> = facts(n)
            .into_iter()
            .map(|(label, value)| (label.to_lowercase(), json!(value)))
            .chain([("output".to_string(), json!(output(n, 4000)))])
            .collect();
        let mut event = json!({
            "routing_key": key,
            "event_action": "trigger",
            "dedup_key": dedup,
            "payload": {
                "summary": cut(&summary(n), 1000),
                "source": n.host,
                "severity": severity,
                "component": n.service,
                "custom_details": details,
            },
            "client": "Watcher",
        });
        if let Some(url) = &n.url {
            event["client_url"] = json!(url);
            event["links"] = json!([{ "href": url, "text": "Watcher" }]);
        }
        event
    };
    let resolve =
        |dedup: &str| json!({ "routing_key": key, "event_action": "resolve", "dedup_key": dedup });
    match n.kind {
        Kind::Problem => Some(vec![trigger(&dedup)]),
        Kind::Recovery => Some(vec![resolve(&dedup)]),
        // A test opens an incident and closes it again.
        Kind::Test => Some(vec![trigger("watcher/test"), resolve("watcher/test")]),
        _ => None,
    }
}

pub(crate) async fn pagerduty(
    http: &reqwest::Client,
    api: &str,
    events: &[Value],
) -> Result<(), Delivery> {
    for event in events {
        send(http.post(api).json(event), "pagerduty").await?;
    }
    Ok(())
}

/// How long a script may run.
const SCRIPT_TIMEOUT: Duration = Duration::from_secs(30);

/// Runs `script` with the notification as JSON on stdin and the main
/// values in `WATCHER_*` environment variables. Exit code 0 is success;
/// a script is not retried, it can retry itself.
pub(crate) async fn script(script: &str, contact: &str, n: &Notification) -> Result<(), Error> {
    let err = |msg: String| Error::Channel("script", msg);
    let argv = match CommandLine::parse(script, false) {
        Ok(CommandLine::Argv(argv)) => argv,
        Ok(_) => return Err(err("expected a program and arguments".into())),
        Err(e) => return Err(err(e.to_string())),
    };
    let json = serde_json::to_vec(n).map_err(|e| err(e.to_string()))?;
    let mut child = tokio::process::Command::new(&argv[0])
        .args(&argv[1..])
        .env("WATCHER_KIND", n.kind.as_str())
        .env("WATCHER_HOST", &n.host)
        .env("WATCHER_SERVICE", &n.service)
        .env("WATCHER_HOST_CHECK", n.host_check.to_string())
        .env("WATCHER_STATE", n.state_name())
        .env("WATCHER_REPEAT", n.repeat.to_string())
        .env("WATCHER_SUBJECT", subject(n))
        .env("WATCHER_SUMMARY", summary(n))
        .env("WATCHER_OUTPUT", &n.output)
        .env("WATCHER_URL", n.url.as_deref().unwrap_or_default())
        .env("WATCHER_CONTACT", contact)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| err(format!("cannot run {}: {e}", argv[0])))?;
    if let Some(mut stdin) = child.stdin.take() {
        // A script that doesn't read stdin closes it early; that's fine.
        let _ = stdin.write_all(&json).await;
    }
    let out = match tokio::time::timeout(SCRIPT_TIMEOUT, child.wait_with_output()).await {
        Ok(out) => out.map_err(|e| err(e.to_string()))?,
        Err(_) => {
            return Err(err(format!(
                "did not finish within {}s",
                SCRIPT_TIMEOUT.as_secs()
            )));
        }
    };
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let line = stderr.lines().next().unwrap_or_default().trim();
    let status = match out.status.code() {
        Some(code) => format!("exit code {code}"),
        None => "killed by a signal".to_string(),
    };
    Err(err(match line.is_empty() {
        true => status,
        false => format!("{status}: {}", cut(line, 200)),
    }))
}
