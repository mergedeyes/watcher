//! Runtime operations: flags on hosts and services (acknowledged, muted,
//! checks off), downtimes, and global settings.

use crate::{Result, Store, epoch, from_epoch};
use serde::Serialize;
use std::time::SystemTime;

/// Something set on a host (`web01`) or a service (`web01/http`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FlagKind {
    /// The problem is known; no more problem notifications until OK.
    Ack,
    /// No notifications.
    Mute,
    /// No active checks.
    NoChecks,
}

impl FlagKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FlagKind::Ack => "ack",
            FlagKind::Mute => "mute",
            FlagKind::NoChecks => "no_checks",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "ack" => FlagKind::Ack,
            "mute" => FlagKind::Mute,
            "no_checks" => FlagKind::NoChecks,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Flag {
    pub kind: FlagKind,
    pub target: String,
    pub author: String,
    pub comment: String,
    #[serde(serialize_with = "crate::query::rfc3339")]
    pub created: SystemTime,
}

/// A planned period without notifications for its target: `kind` is
/// `host`, `service`, `hostgroup`, `servicegroup` or `downtimegroup`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Downtime {
    pub id: i64,
    pub kind: String,
    pub target: String,
    #[serde(serialize_with = "crate::query::rfc3339")]
    pub starts: SystemTime,
    #[serde(serialize_with = "crate::query::rfc3339")]
    pub ends: SystemTime,
    pub author: String,
    pub comment: String,
    #[serde(serialize_with = "crate::query::rfc3339")]
    pub created: SystemTime,
}

type DowntimeRow = (i64, String, String, f64, f64, String, String, f64);

const DOWNTIME_COLUMNS: &str = "id, kind, target, extract(epoch FROM starts)::float8,
    extract(epoch FROM ends)::float8, author, comment, extract(epoch FROM created)::float8";

fn downtime((id, kind, target, starts, ends, author, comment, created): DowntimeRow) -> Downtime {
    Downtime {
        id,
        kind,
        target,
        starts: from_epoch(starts),
        ends: from_epoch(ends),
        author,
        comment,
        created: from_epoch(created),
    }
}

/// An acknowledgement or a downtime starting or ending, for the alert
/// history.
#[derive(Debug, Clone, PartialEq)]
pub struct OpsEvent {
    pub host: String,
    /// `None`: about the host.
    pub service: Option<String>,
    /// `ack`, `downtime_start` or `downtime_end`.
    pub kind: &'static str,
    /// The state at the time: `critical`, `down`, `ok`, ...
    pub state: String,
    /// Who and why, e.g. `alice: on it`.
    pub output: String,
    pub time: SystemTime,
}

impl Store {
    /// Adds `event` to the alert history; `false` if the host or service
    /// isn't known (yet).
    pub async fn add_event(&self, event: &OpsEvent) -> Result<bool> {
        let done = sqlx::query(
            "INSERT INTO state_events
                 (service_id, host_id, ts, kind, from_state, to_state, state_type, hard, output)
             SELECT s.id, CASE WHEN $2::text IS NULL THEN h.id END, to_timestamp($3), $4,
                    NULL, $5, 'hard', false, $6
             FROM hosts h
             LEFT JOIN services s
                    ON s.host_id = h.id AND s.name = $2 AND s.removed_at IS NULL
             WHERE h.name = $1 AND h.removed_at IS NULL
               AND ($2::text IS NULL OR s.id IS NOT NULL)",
        )
        .bind(&event.host)
        .bind(&event.service)
        .bind(epoch(event.time))
        .bind(event.kind)
        .bind(&event.state)
        .bind(&event.output)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn flags(&self) -> Result<Vec<Flag>> {
        let rows: Vec<(String, String, String, String, f64)> = sqlx::query_as(
            "SELECT kind, target, author, comment, extract(epoch FROM created)::float8
             FROM flags ORDER BY created",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(kind, target, author, comment, created)| {
                Some(Flag {
                    kind: FlagKind::parse(&kind)?,
                    target,
                    author,
                    comment,
                    created: from_epoch(created),
                })
            })
            .collect())
    }

    /// Sets a flag, replacing one of the same kind and target.
    pub async fn set_flag(&self, flag: &Flag) -> Result<()> {
        sqlx::query(
            "INSERT INTO flags (kind, target, author, comment, created)
             VALUES ($1, $2, $3, $4, to_timestamp($5))
             ON CONFLICT (kind, target) DO UPDATE
             SET author = EXCLUDED.author, comment = EXCLUDED.comment,
                 created = EXCLUDED.created",
        )
        .bind(flag.kind.as_str())
        .bind(&flag.target)
        .bind(&flag.author)
        .bind(&flag.comment)
        .bind(epoch(flag.created))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Whether there was such a flag.
    pub async fn clear_flag(&self, kind: FlagKind, target: &str) -> Result<bool> {
        let done = sqlx::query("DELETE FROM flags WHERE kind = $1 AND target = $2")
            .bind(kind.as_str())
            .bind(target)
            .execute(&self.pool)
            .await?;
        Ok(done.rows_affected() > 0)
    }

    /// Downtimes that haven't ended and aren't cancelled, by start.
    pub async fn downtimes(&self) -> Result<Vec<Downtime>> {
        let sql = format!(
            "SELECT {DOWNTIME_COLUMNS} FROM downtimes
             WHERE cancelled IS NULL AND ends > now() ORDER BY starts, id"
        );
        let rows: Vec<DowntimeRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(downtime).collect())
    }

    /// Stores a downtime; its `id` and `created` are ignored.
    pub async fn add_downtime(&self, d: &Downtime) -> Result<Downtime> {
        let sql = format!(
            "INSERT INTO downtimes (kind, target, starts, ends, author, comment)
             VALUES ($1, $2, to_timestamp($3), to_timestamp($4), $5, $6)
             RETURNING {DOWNTIME_COLUMNS}"
        );
        let row: DowntimeRow = sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(&d.kind)
            .bind(&d.target)
            .bind(epoch(d.starts))
            .bind(epoch(d.ends))
            .bind(&d.author)
            .bind(&d.comment)
            .fetch_one(&self.pool)
            .await?;
        Ok(downtime(row))
    }

    /// Whether an open downtime with this id existed.
    pub async fn cancel_downtime(&self, id: i64) -> Result<bool> {
        let done = sqlx::query(
            "UPDATE downtimes SET cancelled = now() WHERE id = $1 AND cancelled IS NULL",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn setting(&self, name: &str) -> Result<Option<serde_json::Value>> {
        Ok(
            sqlx::query_scalar("SELECT value FROM settings WHERE name = $1")
                .bind(name)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub async fn set_setting(&self, name: &str, value: &serde_json::Value) -> Result<()> {
        sqlx::query(
            "INSERT INTO settings (name, value) VALUES ($1, $2)
             ON CONFLICT (name) DO UPDATE SET value = EXCLUDED.value",
        )
        .bind(name)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
