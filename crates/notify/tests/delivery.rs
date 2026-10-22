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
use watcher_config::{Contact, Smtp, SmtpTls};
use watcher_core::State;
use watcher_notify::{Kind, Notification, Notifier};

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
        url: None,
    }
}

fn contact(email: &[&str], webhook: Option<String>) -> Contact {
    Contact {
        name: "ops".into(),
        email: email.iter().map(|s| s.to_string()).collect(),
        webhook,
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
