//! Delivery over real channels: webhooks against a local server, email
//! against Mailpit when `WATCHER_TEST_MAILPIT` is set (see compose.yaml),
//! e.g. `WATCHER_TEST_MAILPIT=localhost`.

use axum::Json;
use axum::extract::State as AxState;
use axum::http::StatusCode;
use axum::routing::post;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use watcher_config::{Channels, Contact, Gotify, Ntfy, Smtp, SmtpTls, Telegram};
use watcher_core::State;
use watcher_notify::{Endpoints, Kind, Notification, Notifier};

fn notification(service: &str) -> Notification {
    Notification {
        kind: Kind::Problem,
        host: "web01".into(),
        address: "10.0.0.5".into(),
        service: service.into(),
        host_check: false,
        state: State::Critical,
        previous_state: Some(State::Ok),
        attempts: 3,
        output: "DISK CRITICAL - free space: / 120 MB (2%)".into(),
        long_output: String::new(),
        time: SystemTime::now(),
        since: None,
        repeat: false,
        url: None,
        instance: None,
    }
}

fn contact(email: &[&str], webhook: Option<String>) -> Contact {
    Contact {
        name: "ops".into(),
        email: email.iter().map(|s| s.to_string()).collect(),
        webhook,
        channels: Default::default(),
        groups: Vec::new(),
        origin: Default::default(),
    }
}

/// Received bodies, and the statuses still to answer with.
type Recv = (Arc<Mutex<Vec<Value>>>, Arc<Mutex<std::vec::IntoIter<u16>>>);

/// A webhook receiver answering with `statuses` in turn (then 200) and
/// recording every body.
async fn receiver(statuses: Vec<u16>) -> (String, Arc<Mutex<Vec<Value>>>) {
    let received = Arc::new(Mutex::new(Vec::new()));
    let statuses = Arc::new(Mutex::new(statuses.into_iter()));
    let state = (received.clone(), statuses);
    let app = axum::Router::new()
        .route(
            "/hook",
            post(
                |AxState((received, statuses)): AxState<Recv>, Json(body): Json<Value>| async move {
                    received.lock().unwrap().push(body);
                    let code = statuses.lock().unwrap().next().unwrap_or(200);
                    StatusCode::from_u16(code).unwrap()
                },
            ),
        )
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook", listener.local_addr().unwrap());
    tokio::spawn(axum::serve(listener, app).into_future());
    (url, received)
}

#[tokio::test]
async fn webhook_retries_server_errors() {
    let (url, received) = receiver(vec![503]).await;
    let notifier = Notifier::new(None, None).unwrap();
    let results = notifier
        .deliver(&contact(&[], Some(url)), &notification("disk"))
        .await;
    assert_eq!(results.len(), 1);
    assert!(results[0].1.is_ok(), "{:?}", results[0].1);

    let bodies = received.lock().unwrap();
    assert_eq!(bodies.len(), 2, "one failed attempt, one retry");
    assert_eq!(bodies[1]["kind"], "problem");
    assert_eq!(bodies[1]["service"], "disk");
    assert_eq!(bodies[1]["previous_state"], "ok");
}

#[tokio::test]
async fn webhook_client_errors_are_not_retried() {
    let (url, received) = receiver(vec![404]).await;
    let notifier = Notifier::new(None, None).unwrap();
    let results = notifier
        .deliver(&contact(&[], Some(url)), &notification("disk"))
        .await;
    let err = results[0].1.as_ref().unwrap_err().to_string();
    assert!(err.contains("404"), "{err}");
    assert_eq!(received.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn email_without_smtp_fails_clearly() {
    let notifier = Notifier::new(None, None).unwrap();
    let results = notifier
        .deliver(&contact(&["ops@example.com"], None), &notification("disk"))
        .await;
    assert_eq!(results[0].0, "email");
    assert!(
        results[0]
            .1
            .as_ref()
            .unwrap_err()
            .to_string()
            .contains("no [smtp]")
    );
}

#[tokio::test]
async fn email_through_mailpit() {
    let Ok(host) = std::env::var("WATCHER_TEST_MAILPIT") else {
        eprintln!("skipped: set WATCHER_TEST_MAILPIT");
        return;
    };
    let smtp = Smtp {
        host: host.clone(),
        port: Some(1025),
        tls: SmtpTls::None,
        username: None,
        password: None,
        password_file: None,
        from: "Watcher <watcher@example.com>".into(),
    };
    let notifier = Notifier::new(Some(&smtp), None).unwrap();
    // A unique service name to find this message among others.
    let service = format!("disk-{}", std::process::id());
    let results = notifier
        .deliver(
            &contact(&["ops@example.com", "boss@example.com"], None),
            &notification(&service),
        )
        .await;
    assert!(results[0].1.is_ok(), "{:?}", results[0].1);

    let subject = format!("[Watcher] PROBLEM: web01/{service} is CRITICAL");
    let search = format!("http://{host}:8025/api/v1/search?query=subject:%22{service}%22");
    let found: Value = reqwest::get(&search).await.unwrap().json().await.unwrap();
    let message = &found["messages"][0];
    assert_eq!(message["Subject"], subject);
    assert_eq!(message["To"].as_array().unwrap().len(), 2);
    assert_eq!(message["From"]["Address"], "watcher@example.com");
}

/// A request a service got: path, headers, JSON body.
#[derive(Debug, Clone)]
struct Got {
    path: String,
    headers: axum::http::HeaderMap,
    body: Value,
}

type Answers = Arc<Mutex<std::vec::IntoIter<(u16, String)>>>;

/// Stands in for every service: records each request on any path and
/// answers with `answers` in turn (then 200).
async fn services(answers: Vec<(u16, &str)>) -> (String, Arc<Mutex<Vec<Got>>>) {
    let got = Arc::new(Mutex::new(Vec::new()));
    let answers: Answers = Arc::new(Mutex::new(
        answers
            .into_iter()
            .map(|(c, t)| (c, t.to_string()))
            .collect::<Vec<_>>()
            .into_iter(),
    ));
    let state = (got.clone(), answers);
    let app = axum::Router::new()
        .fallback(
            |AxState((got, answers)): AxState<(Arc<Mutex<Vec<Got>>>, Answers)>,
             uri: axum::http::Uri,
             headers: axum::http::HeaderMap,
             body: axum::body::Bytes| async move {
                got.lock().unwrap().push(Got {
                    path: uri.path().to_string(),
                    headers,
                    body: serde_json::from_slice(&body).unwrap_or(Value::Null),
                });
                let (code, text) = answers
                    .lock()
                    .unwrap()
                    .next()
                    .unwrap_or((200, String::new()));
                (StatusCode::from_u16(code).unwrap(), text)
            },
        )
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(axum::serve(listener, app).into_future());
    (base, got)
}

fn everywhere(base: &str) -> Contact {
    let mut c = contact(&[], None);
    c.channels = Channels {
        slack: Some(format!("{base}/slack")),
        teams: Some(format!("{base}/teams")),
        telegram: Some(Telegram {
            token: "123:abc".into(),
            chat: "-100".into(),
        }),
        ntfy: Some(Ntfy {
            url: format!("{base}/alerts"),
            token: Some("tk".into()),
        }),
        gotify: Some(Gotify {
            url: format!("{base}/gotify"),
            token: "gk".into(),
        }),
        pagerduty: Some("KEY".into()),
        jira: None,
        script: None,
    };
    c
}

fn notifier(base: &str) -> Notifier {
    Notifier::new(None, None)
        .unwrap()
        .with_endpoints(Endpoints {
            telegram: base.to_string(),
            pagerduty: format!("{base}/pd"),
        })
}

#[tokio::test]
async fn every_channel_gets_its_own_format() {
    let (base, got) = services(vec![]).await;
    let mut n = notification("disk");
    n.url = Some("https://w.example/services/web01/disk".into());
    let results = notifier(&base).deliver(&everywhere(&base), &n).await;
    let names: Vec<&str> = results.iter().map(|(c, _)| *c).collect();
    assert_eq!(
        names,
        ["slack", "teams", "telegram", "ntfy", "gotify", "pagerduty"]
    );
    for (channel, r) in &results {
        assert!(r.is_ok(), "{channel}: {r:?}");
    }
    let got = got.lock().unwrap().clone();
    let at = |path: &str| got.iter().find(|g| g.path == path).unwrap().clone();

    let slack = at("/slack").body;
    assert_eq!(slack["text"], "[Watcher] PROBLEM: web01/disk is CRITICAL");
    assert_eq!(slack["attachments"][0]["color"], "#e5484d");
    let text = slack["attachments"][0]["text"].as_str().unwrap();
    assert!(
        text.starts_with("*web01/disk is CRITICAL (was OK)*\nDISK CRITICAL"),
        "{text}"
    );

    let teams = at("/teams").body;
    let card = &teams["attachments"][0];
    assert_eq!(
        card["contentType"],
        "application/vnd.microsoft.card.adaptive"
    );
    assert_eq!(card["content"]["body"][0]["color"], "Attention");
    assert_eq!(card["content"]["actions"][0]["url"], n.url.clone().unwrap());

    let telegram = at("/bot123:abc/sendMessage").body;
    assert_eq!(telegram["chat_id"], "-100");
    assert_eq!(telegram["parse_mode"], "HTML");
    let text = telegram["text"].as_str().unwrap();
    assert!(
        text.starts_with("<b>web01/disk is CRITICAL (was OK)</b>\n\n<pre>DISK"),
        "{text}"
    );
    assert!(
        text.ends_with("<a href=\"https://w.example/services/web01/disk\">Details</a>"),
        "{text}"
    );

    // ntfy: JSON to the server, the topic inside.
    let ntfy = at("/");
    assert_eq!(ntfy.body["topic"], "alerts");
    assert_eq!(ntfy.body["priority"], 5);
    assert_eq!(ntfy.body["click"], n.url.clone().unwrap());
    assert_eq!(ntfy.headers["authorization"], "Bearer tk");

    let gotify = at("/gotify/message");
    assert_eq!(gotify.headers["x-gotify-key"], "gk");
    assert_eq!(gotify.body["priority"], 8);
    assert_eq!(gotify.body["title"], "web01/disk is CRITICAL (was OK)");

    let pd = at("/pd").body;
    assert_eq!(pd["routing_key"], "KEY");
    assert_eq!(pd["event_action"], "trigger");
    assert_eq!(pd["dedup_key"], "watcher/web01/disk");
    assert_eq!(pd["payload"]["severity"], "critical");
    assert_eq!(pd["payload"]["source"], "web01");
}

#[tokio::test]
async fn pagerduty_resolves_and_skips_other_kinds() {
    let (base, got) = services(vec![]).await;
    let mut c = contact(&[], None);
    c.channels.pagerduty = Some("KEY".into());
    let notifier = notifier(&base);
    let mut n = notification("disk");
    n.kind = Kind::Recovery;
    n.state = State::Ok;
    let results = notifier.deliver(&c, &n).await;
    assert!(results[0].1.is_ok());
    assert_eq!(got.lock().unwrap()[0].body["event_action"], "resolve");
    assert_eq!(
        got.lock().unwrap()[0].body["dedup_key"],
        "watcher/web01/disk"
    );

    n.kind = Kind::DowntimeStart;
    assert!(notifier.deliver(&c, &n).await.is_empty());
    assert_eq!(got.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn service_errors_are_reported_without_tokens() {
    let (base, got) = services(vec![(
        400,
        r#"{"ok":false,"error_code":400,"description":"Bad Request: chat not found"}"#,
    )])
    .await;
    let mut c = contact(&[], None);
    c.channels.telegram = Some(Telegram {
        token: "123:secret".into(),
        chat: "-1".into(),
    });
    let results = notifier(&base).deliver(&c, &notification("disk")).await;
    let err = results[0].1.as_ref().unwrap_err().to_string();
    assert!(
        err.starts_with("telegram: answered 400 Bad Request: "),
        "{err}"
    );
    assert!(err.contains("chat not found"), "{err}");
    assert!(!err.contains("secret"), "{err}");
    // A client error is not retried.
    assert_eq!(got.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn scripts_get_json_and_environment() {
    let dir = std::env::temp_dir().join(format!("watcher-notify-script-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("out");
    let script = dir.join("notify-me");
    // Written by a child process: a file this process has open for writing
    // can't be run while other tests fork ("Text file busy").
    let written = std::process::Command::new("sh")
        .arg("-c")
        .arg(r#"printf '%s\n' '#!/bin/sh' 'cat > "$1"' 'echo "$WATCHER_KIND $WATCHER_HOST/$WATCHER_SERVICE $WATCHER_STATE $WATCHER_CONTACT" >> "$1"' > "$0" && chmod 755 "$0""#)
        .arg(&script)
        .status()
        .unwrap();
    assert!(written.success());
    let mut c = contact(&[], None);
    c.channels.script = Some(format!("{} {}", script.display(), out.display()));
    let results = Notifier::new(None, None)
        .unwrap()
        .deliver(&c, &notification("disk"))
        .await;
    assert!(results[0].1.is_ok(), "{:?}", results[0].1);
    let text = std::fs::read_to_string(&out).unwrap();
    let (json, env) = text.rsplit_once('}').unwrap();
    let json: Value = serde_json::from_str(&format!("{json}}}")).unwrap();
    assert_eq!(json["service"], "disk");
    assert_eq!(env.trim(), "PROBLEM web01/disk CRITICAL ops");

    c.channels.script = Some("sh -c 'echo no modem >&2; exit 3'".into());
    let results = Notifier::new(None, None)
        .unwrap()
        .deliver(&c, &notification("disk"))
        .await;
    let err = results[0].1.as_ref().unwrap_err().to_string();
    assert_eq!(err, "script: exit code 3: no modem");
    std::fs::remove_dir_all(dir).unwrap();
}

/// What the mock Jira saw and holds: an open issue, created issues,
/// comments and run transitions.
#[derive(Default)]
struct JiraState {
    open_issue: Option<String>,
    searches: Vec<String>,
    created: Vec<Value>,
    comments: Vec<(String, Value)>,
    transitioned: Vec<(String, String)>,
    auth: Vec<String>,
}

type JiraShared = Arc<Mutex<JiraState>>;

/// A little Jira answering the endpoints the channel uses, v2 and v3.
async fn jira_server(open_issue: Option<&str>) -> (String, JiraShared) {
    use axum::extract::{Path, RawQuery};
    use axum::http::HeaderMap;
    use axum::routing::get;
    let state = Arc::new(Mutex::new(JiraState {
        open_issue: open_issue.map(String::from),
        ..Default::default()
    }));
    let record_auth = |state: &JiraShared, headers: &HeaderMap| {
        let auth = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        state.lock().unwrap().auth.push(auth);
    };
    let search = {
        let state = state.clone();
        move |headers: HeaderMap, RawQuery(q): RawQuery| {
            let state = state.clone();
            async move {
                record_auth(&state, &headers);
                let mut s = state.lock().unwrap();
                s.searches.push(q.unwrap_or_default());
                let issues: Vec<Value> = s
                    .open_issue
                    .iter()
                    .map(|k| serde_json::json!({ "key": k }))
                    .collect();
                Json(serde_json::json!({ "issues": issues }))
            }
        }
    };
    let create = {
        let state = state.clone();
        move |headers: HeaderMap, Json(body): Json<Value>| {
            let state = state.clone();
            async move {
                record_auth(&state, &headers);
                let mut s = state.lock().unwrap();
                s.created.push(body);
                s.open_issue = Some("OPS-1".into());
                Json(serde_json::json!({ "id": "10001", "key": "OPS-1" }))
            }
        }
    };
    let comment = {
        let state = state.clone();
        move |Path(key): Path<String>, Json(body): Json<Value>| {
            let state = state.clone();
            async move {
                state.lock().unwrap().comments.push((key, body));
                (StatusCode::CREATED, Json(serde_json::json!({})))
            }
        }
    };
    let transitions = {
        let state = state.clone();
        move |Path(key): Path<String>| {
            let _ = (&state, key);
            async move {
                Json(serde_json::json!({ "transitions": [
                    { "id": "11", "name": "In Progress", "to": { "name": "In Progress" } },
                    { "id": "31", "name": "Finish", "to": { "name": "Done" } },
                ] }))
            }
        }
    };
    let run_transition = {
        let state = state.clone();
        move |Path(key): Path<String>, Json(body): Json<Value>| {
            let state = state.clone();
            async move {
                let id = body["transition"]["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                state.lock().unwrap().transitioned.push((key, id));
                StatusCode::NO_CONTENT
            }
        }
    };
    let app = axum::Router::new()
        .route("/rest/api/3/search/jql", get(search.clone()))
        .route("/rest/api/2/search", get(search))
        .route("/rest/api/3/issue", post(create.clone()))
        .route("/rest/api/2/issue", post(create))
        .route("/rest/api/3/issue/{key}/comment", post(comment.clone()))
        .route("/rest/api/2/issue/{key}/comment", post(comment))
        .route(
            "/rest/api/3/issue/{key}/transitions",
            get(transitions.clone()).post(run_transition.clone()),
        )
        .route(
            "/rest/api/2/issue/{key}/transitions",
            get(transitions).post(run_transition),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(axum::serve(listener, app).into_future());
    (url, state)
}

fn jira_contact(url: &str, cloud: bool, close: Option<&str>) -> Contact {
    let mut c = contact(&[], None);
    c.channels.jira = Some(watcher_config::Jira {
        url: url.to_string(),
        project: "OPS".into(),
        cloud,
        email: cloud.then(|| "bot@example.com".to_string()),
        token: "t0ken".into(),
        issue_type: "Task".into(),
        close: close.map(String::from),
    });
    c
}

#[tokio::test]
async fn jira_creates_one_issue_per_problem() {
    let (url, state) = jira_server(None).await;
    let notifier = Notifier::new(None, None).unwrap();
    let c = jira_contact(&url, true, Some("Done"));

    // No open issue yet: the problem creates one, over v3 with Basic auth.
    let results = notifier.deliver(&c, &notification("disk")).await;
    assert_eq!(results.len(), 1);
    assert!(results[0].1.is_ok(), "{:?}", results[0].1);
    {
        let s = state.lock().unwrap();
        assert_eq!(s.created.len(), 1);
        let fields = &s.created[0]["fields"];
        assert_eq!(fields["project"]["key"], "OPS");
        assert_eq!(fields["issuetype"]["name"], "Task");
        assert_eq!(fields["summary"], "web01/disk is CRITICAL (was OK)");
        let labels = fields["labels"].as_array().unwrap();
        assert_eq!(labels[0], "watcher");
        assert!(labels[1].as_str().unwrap().starts_with("watcher-"));
        // Cloud: ADF description, Basic auth, the v3 search endpoint.
        assert_eq!(fields["description"]["type"], "doc");
        assert!(
            s.auth.iter().all(|a| a.starts_with("Basic ")),
            "{:?}",
            s.auth
        );
        assert!(s.searches[0].contains("labels"));
        assert!(s.comments.is_empty());
    }

    // A repeat while the issue is open comments instead.
    let mut again = notification("disk");
    again.repeat = true;
    again.since = Some(SystemTime::now());
    let results = notifier.deliver(&c, &again).await;
    assert!(results[0].1.is_ok(), "{:?}", results[0].1);
    {
        let s = state.lock().unwrap();
        assert_eq!(s.created.len(), 1);
        assert_eq!(s.comments.len(), 1);
        assert_eq!(s.comments[0].0, "OPS-1");
        assert!(s.transitioned.is_empty());
    }

    // The recovery comments and runs the close transition, matched by
    // the status it leads to ("Done" is transition 31, "Finish").
    let mut ok = notification("disk");
    ok.kind = Kind::Recovery;
    ok.state = State::Ok;
    ok.previous_state = Some(State::Critical);
    ok.output = "DISK OK".into();
    let results = notifier.deliver(&c, &ok).await;
    assert!(results[0].1.is_ok(), "{:?}", results[0].1);
    let s = state.lock().unwrap();
    assert_eq!(s.comments.len(), 2);
    assert_eq!(
        s.transitioned.as_slice(),
        [("OPS-1".to_string(), "31".to_string())]
    );
}

#[tokio::test]
async fn jira_data_center_uses_v2_and_a_token() {
    let (url, state) = jira_server(Some("OPS-7")).await;
    let notifier = Notifier::new(None, None).unwrap();
    let c = jira_contact(&url, false, None);

    // An open issue exists: the problem only comments, over v2 with a
    // bearer token and a text body.
    let results = notifier.deliver(&c, &notification("disk")).await;
    assert!(results[0].1.is_ok(), "{:?}", results[0].1);
    {
        let s = state.lock().unwrap();
        assert!(s.created.is_empty());
        assert_eq!(s.comments[0].0, "OPS-7");
        let body = s.comments[0].1["body"].as_str().unwrap();
        assert!(body.contains("{code}"), "{body}");
        assert!(
            s.auth.iter().all(|a| a.starts_with("Bearer ")),
            "{:?}",
            s.auth
        );
    }

    // A recovery without `close` only comments; one for a problem nobody
    // has an issue for does nothing.
    let mut ok = notification("disk");
    ok.kind = Kind::Recovery;
    ok.state = State::Ok;
    let results = notifier.deliver(&c, &ok).await;
    assert!(results[0].1.is_ok(), "{:?}", results[0].1);
    {
        let mut s = state.lock().unwrap();
        assert_eq!(s.comments.len(), 2);
        assert!(s.transitioned.is_empty());
        s.open_issue = None;
    }
    let results = notifier.deliver(&c, &ok).await;
    assert!(results[0].1.is_ok(), "{:?}", results[0].1);
    assert_eq!(state.lock().unwrap().comments.len(), 2);

    // Flapping never becomes a ticket.
    let mut flap = notification("disk");
    flap.kind = Kind::FlappingStart;
    let results = notifier.deliver(&c, &flap).await;
    assert!(results.is_empty());
}
