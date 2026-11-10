//! Jira issues as a notification channel: one issue per open problem,
//! found again by a label, so repeats and escalations add comments
//! instead of new issues — across restarts, with or without a database.
//! The recovery adds a comment and, with `close` set, runs that
//! transition. Jira Cloud speaks REST v3 (ADF bodies, email + API
//! token), Data Center/Server REST v2 (wiki markup, personal access
//! token).

use crate::channels::{cut, output};
use crate::email::{facts, summary};
use crate::webhook::reason;
use crate::{Delivery, Error, Kind, Notification};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use watcher_config::Jira;

/// Whether this kind goes to Jira at all. Flapping and downtimes don't
/// become tickets.
pub(crate) fn takes(kind: Kind) -> bool {
    matches!(kind, Kind::Problem | Kind::Recovery | Kind::Test)
}

/// Creates, comments on or closes the issue for `n`.
pub(crate) async fn deliver(
    http: &reqwest::Client,
    j: &Jira,
    n: &Notification,
) -> Result<(), Delivery> {
    if n.kind == Kind::Test {
        // A test proves the whole path: create an issue and, with
        // `close` set, run the transition on it.
        let key = create(http, j, n).await?;
        if j.close.is_some() {
            transition(http, j, &key).await?;
        }
        return Ok(());
    }
    let open = find_open(http, j, &label(n)).await?;
    match (n.kind, open) {
        // A state change or repeat while the issue is open only comments.
        (Kind::Problem, Some(key)) => comment(http, j, &key, n).await,
        (Kind::Problem, None) => create(http, j, n).await.map(|_| ()),
        (Kind::Recovery, Some(key)) => {
            comment(http, j, &key, n).await?;
            if j.close.is_some() {
                transition(http, j, &key).await?;
            }
            Ok(())
        }
        // Recovered, but no open issue (closed by hand, or the problem
        // was never created): nothing to do.
        (Kind::Recovery, None) => Ok(()),
        _ => Ok(()),
    }
}

/// `2` for Data Center/Server, `3` for Cloud.
fn api(j: &Jira) -> &'static str {
    if j.cloud { "3" } else { "2" }
}

fn authed(req: reqwest::RequestBuilder, j: &Jira) -> reqwest::RequestBuilder {
    match (&j.email, j.cloud) {
        (Some(email), true) => req.basic_auth(email, Some(&j.token)),
        _ => req.bearer_auth(&j.token),
    }
}

/// The label that finds the problem's issue again: a hash, since labels
/// can't hold spaces or arbitrary characters.
fn label(n: &Notification) -> String {
    let id = match &n.instance {
        Some(name) => format!("watcher/{name}/{}", n.service_id()),
        None => format!("watcher/{}", n.service_id()),
    };
    let hash = Sha256::digest(id.as_bytes());
    let hex: String = hash[..12].iter().map(|b| format!("{b:02x}")).collect();
    format!("watcher-{hex}")
}

/// Sends a request and parses the answer; an empty answer (204) is
/// `Null`. 429 and 5xx are worth retrying. The answer is kept in errors
/// since it says what is wrong; URLs are left out (tokens).
async fn call(req: reqwest::RequestBuilder) -> Result<Value, Delivery> {
    let err = |msg: String| Error::Channel("jira", msg);
    let resp = req
        .send()
        .await
        .map_err(|e| Delivery::Transient(err(reason(&e.without_url()))))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if status.is_success() {
        return Ok(serde_json::from_str(&text).unwrap_or(Value::Null));
    }
    let answer = cut(text.trim(), 200);
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

/// The key of the open issue with `label`, if there is one.
async fn find_open(
    http: &reqwest::Client,
    j: &Jira,
    label: &str,
) -> Result<Option<String>, Delivery> {
    let jql = format!(
        "project = \"{}\" AND labels = \"{label}\" AND statusCategory != Done \
         ORDER BY created DESC",
        j.project.replace(['\\', '"'], "")
    );
    // Cloud retired the old search endpoint in favour of /search/jql.
    let path = match j.cloud {
        true => format!("{}/rest/api/3/search/jql", j.url),
        false => format!("{}/rest/api/2/search", j.url),
    };
    let req = http.get(path).query(&[
        ("jql", jql.as_str()),
        ("maxResults", "1"),
        ("fields", "key"),
    ]);
    let answer = call(authed(req, j)).await?;
    Ok(answer["issues"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|i| i["key"].as_str())
        .map(String::from))
}

/// Creates the issue for `n` and returns its key.
async fn create(http: &reqwest::Client, j: &Jira, n: &Notification) -> Result<String, Delivery> {
    let fields = json!({
        "project": { "key": j.project },
        "issuetype": { "name": j.issue_type },
        // Jira limits summaries to 255 characters, without line breaks.
        "summary": cut(&summary(n).replace('\n', " "), 250),
        "labels": ["watcher", label(n)],
        "description": description(j, n),
    });
    let req = http
        .post(format!("{}/rest/api/{}/issue", j.url, api(j)))
        .json(&json!({ "fields": fields }));
    let answer = call(authed(req, j)).await?;
    match answer["key"].as_str() {
        Some(key) => Ok(key.to_string()),
        None => Err(Delivery::Permanent(Error::Channel(
            "jira",
            "the created issue came back without a key".into(),
        ))),
    }
}

async fn comment(
    http: &reqwest::Client,
    j: &Jira,
    key: &str,
    n: &Notification,
) -> Result<(), Delivery> {
    let req = http
        .post(format!("{}/rest/api/{}/issue/{key}/comment", j.url, api(j)))
        .json(&json!({ "body": description(j, n) }));
    call(authed(req, j)).await.map(|_| ())
}

/// Runs the `close` transition on `key`, found by its name or the status
/// it leads to.
async fn transition(http: &reqwest::Client, j: &Jira, key: &str) -> Result<(), Delivery> {
    let close = j.close.as_deref().unwrap_or_default();
    let url = format!("{}/rest/api/{}/issue/{key}/transitions", j.url, api(j));
    let answer = call(authed(http.get(&url), j)).await?;
    let transitions = answer["transitions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let matches = |v: &Value| v.as_str().is_some_and(|s| s.eq_ignore_ascii_case(close));
    let found = transitions
        .iter()
        .find(|t| matches(&t["name"]) || matches(&t["to"]["name"]));
    let Some(id) = found.and_then(|t| t["id"].as_str()) else {
        let names: Vec<&str> = transitions
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        return Err(Delivery::Permanent(Error::Channel(
            "jira",
            format!(
                "{key} has no transition {close:?} (available: {})",
                names.join(", ")
            ),
        )));
    };
    let req = http.post(&url).json(&json!({ "transition": { "id": id } }));
    call(authed(req, j)).await.map(|_| ())
}

/// The issue description or comment: the summary, the output, and the
/// facts. Cloud (v3) wants Atlassian Document Format, Data Center (v2)
/// takes wiki markup.
fn description(j: &Jira, n: &Notification) -> Value {
    let out = output(n, 4000);
    let lines: Vec<String> = facts(n)
        .into_iter()
        .map(|(label, value)| format!("{label}: {value}"))
        .collect();
    if !j.cloud {
        let mut text = summary(n);
        if !out.is_empty() {
            text += &format!("\n\n{{code}}\n{out}\n{{code}}");
        }
        text += &format!("\n\n{}", lines.join("\n"));
        return json!(text);
    }
    let paragraph =
        |text: &str| json!({ "type": "paragraph", "content": [{ "type": "text", "text": text }] });
    let mut content = vec![paragraph(&summary(n))];
    if !out.is_empty() {
        content.push(json!({
            "type": "codeBlock",
            "content": [{ "type": "text", "text": out }],
        }));
    }
    // One paragraph per fact: `\n` inside an ADF text node is not a line
    // break.
    content.extend(lines.iter().map(|l| paragraph(l)));
    json!({ "type": "doc", "version": 1, "content": content })
}
