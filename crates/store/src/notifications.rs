//! The notification log: who got which notification, when, and whether it
//! was delivered.

use crate::{Result, Store, epoch, from_epoch};
use serde::Serialize;
use std::time::{Duration, SystemTime};

/// One notification to one contact over one channel.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SentNotification {
    #[serde(serialize_with = "crate::query::rfc3339")]
    pub ts: SystemTime,
    pub host: String,
    /// For host notifications: the host check.
    pub service: String,
    pub host_check: bool,
    /// `problem`, `recovery`, `flappingstart`, `flappingstop`,
    /// `downtimestart` or `downtimeend`.
    pub kind: String,
    /// `ok`, `warning`, `critical`, `unknown`, `up` or `down`.
    pub state: String,
    /// A problem sent again: a repeat or an escalation.
    pub repeat: bool,
    #[serde(serialize_with = "crate::query::rfc3339_opt")]
    pub since: Option<SystemTime>,
    pub contact: String,
    /// `email`, `webhook`, `slack`, ... or empty if the contact has none.
    pub channel: String,
    /// `None`: delivered.
    pub error: Option<String>,
}

/// Which log entries to list.
#[derive(Debug, Clone, Default)]
pub struct NotificationFilter {
    pub host: Option<String>,
    pub service: Option<String>,
    pub contact: Option<String>,
    /// Only host notifications (`true`) or only service notifications.
    pub host_check: Option<bool>,
    pub before: Option<SystemTime>,
    pub since: Option<SystemTime>,
    /// Only failed deliveries.
    pub failed: bool,
    pub limit: i64,
}

/// The setting that holds when the log started: problems that began
/// earlier may have been notified without an entry.
const LOG_START: &str = "notification_log_start";

type Row = (
    f64,
    String,
    String,
    bool,
    String,
    String,
    bool,
    Option<f64>,
    String,
    String,
    Option<String>,
);

impl Store {
    pub async fn add_notifications(&self, rows: &[SentNotification]) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let col = |f: fn(&SentNotification) -> String| rows.iter().map(f).collect::<Vec<_>>();
        sqlx::query(
            "INSERT INTO notifications
                 (ts, host, service, host_check, kind, state, repeat, since, contact,
                  channel, error)
             SELECT to_timestamp(ts), host, service, host_check, kind, state, repeat,
                    to_timestamp(since), contact, channel, error
             FROM unnest($1::float8[], $2::text[], $3::text[], $4::bool[], $5::text[],
                         $6::text[], $7::bool[], $8::float8[], $9::text[], $10::text[],
                         $11::text[])
                  AS t(ts, host, service, host_check, kind, state, repeat, since, contact,
                       channel, error)",
        )
        .bind(rows.iter().map(|r| epoch(r.ts)).collect::<Vec<_>>())
        .bind(col(|r| r.host.clone()))
        .bind(col(|r| r.service.clone()))
        .bind(rows.iter().map(|r| r.host_check).collect::<Vec<_>>())
        .bind(col(|r| r.kind.clone()))
        .bind(col(|r| r.state.clone()))
        .bind(rows.iter().map(|r| r.repeat).collect::<Vec<_>>())
        .bind(rows.iter().map(|r| r.since.map(epoch)).collect::<Vec<_>>())
        .bind(col(|r| r.contact.clone()))
        .bind(col(|r| r.channel.clone()))
        .bind(rows.iter().map(|r| r.error.clone()).collect::<Vec<_>>())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Log entries, newest first.
    pub async fn notifications(&self, f: &NotificationFilter) -> Result<Vec<SentNotification>> {
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT extract(epoch FROM ts)::float8, host, service, host_check, kind, state,
                    repeat, extract(epoch FROM since)::float8, contact, channel, error
             FROM notifications
             WHERE ($1::text IS NULL OR host = $1)
               AND ($2::text IS NULL OR service = $2)
               AND ($3::text IS NULL OR contact = $3)
               AND ($4::float8 IS NULL OR ts < to_timestamp($4))
               AND ($5::float8 IS NULL OR ts >= to_timestamp($5))
               AND (NOT $6 OR error IS NOT NULL)
               AND ($7::bool IS NULL OR host_check = $7)
             ORDER BY ts DESC, id DESC
             LIMIT $8",
        )
        .bind(&f.host)
        .bind(&f.service)
        .bind(&f.contact)
        .bind(f.before.map(epoch))
        .bind(f.since.map(epoch))
        .bind(f.failed)
        .bind(f.host_check)
        .bind(f.limit.clamp(1, 10_000))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(row).collect())
    }

    /// The last PROBLEM each contact got about each host or service within
    /// `within`: `(host/service, contact, time)`, for the repeat clock.
    pub async fn last_problems(
        &self,
        within: Duration,
    ) -> Result<Vec<(String, String, SystemTime)>> {
        let rows: Vec<(String, String, String, f64)> = sqlx::query_as(
            "SELECT DISTINCT ON (host, service, contact)
                    host, service, contact, extract(epoch FROM ts)::float8
             FROM notifications
             WHERE kind = 'problem' AND ts >= to_timestamp($1)
             ORDER BY host, service, contact, ts DESC",
        )
        .bind(epoch(SystemTime::now() - within))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(host, service, contact, ts)| {
                (format!("{host}/{service}"), contact, from_epoch(ts))
            })
            .collect())
    }

    /// When the log started; set to now on the first call.
    pub async fn notification_log_start(&self) -> Result<SystemTime> {
        sqlx::query(
            "INSERT INTO settings (name, value) VALUES ($1, to_jsonb(extract(epoch FROM now())))
             ON CONFLICT (name) DO NOTHING",
        )
        .bind(LOG_START)
        .execute(&self.pool)
        .await?;
        let start: f64 =
            sqlx::query_scalar("SELECT (value #>> '{}')::float8 FROM settings WHERE name = $1")
                .bind(LOG_START)
                .fetch_one(&self.pool)
                .await?;
        Ok(from_epoch(start))
    }

    /// Deletes entries older than `keep`; the number deleted.
    pub async fn purge_notifications(&self, keep: Duration) -> Result<u64> {
        let done = sqlx::query("DELETE FROM notifications WHERE ts < to_timestamp($1)")
            .bind(epoch(SystemTime::now() - keep))
            .execute(&self.pool)
            .await?;
        Ok(done.rows_affected())
    }
}

fn row(
    (ts, host, service, host_check, kind, state, repeat, since, contact, channel, error): Row,
) -> SentNotification {
    SentNotification {
        ts: from_epoch(ts),
        host,
        service,
        host_check,
        kind,
        state,
        repeat,
        since: since.map(from_epoch),
        contact,
        channel,
        error,
    }
}
