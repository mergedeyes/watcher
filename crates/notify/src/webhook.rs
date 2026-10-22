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
        .map_err(|e| Delivery::Transient(Error::Webhook(e.without_url().to_string())))?;
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
