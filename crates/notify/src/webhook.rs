use crate::{Delivery, Error, Notification};

/// POSTs the notification as JSON. 2xx is success; 429 and 5xx are worth
/// retrying, other statuses are not.
pub(crate) async fn post(
    http: &reqwest::Client,
    url: &str,
    n: &Notification,
) -> Result<(), Delivery> {
    let resp = http
        .post(url)
        .json(n)
        .send()
        .await
        .map_err(|e| Delivery::Transient(Error::Webhook(reason(&e.without_url()))))?;
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let err = Error::Webhook(format!("{url} answered {status}"));
    if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        Err(Delivery::Transient(err))
    } else {
        Err(Delivery::Permanent(err))
    }
}

/// The error with its causes: reqwest's own message is only "error sending
/// request", the cause ("Connection refused") is in its sources.
pub(crate) fn reason(err: &dyn std::error::Error) -> String {
    let mut out = err.to_string();
    let mut source = err.source();
    while let Some(s) = source {
        let text = s.to_string();
        if !out.contains(&text) {
            out += &format!(": {text}");
        }
        source = s.source();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn errors_name_their_cause() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let n = Notification::test("ops", None);
        let Err(Delivery::Transient(Error::Webhook(text))) =
            post(&reqwest::Client::new(), "http://127.0.0.1:1/", &n).await
        else {
            panic!("expected a transient error");
        };
        assert!(text.starts_with("error sending request: "), "{text}");
        assert!(text.to_lowercase().contains("refused"), "{text}");
    }
}
