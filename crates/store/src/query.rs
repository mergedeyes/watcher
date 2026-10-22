//! Read queries for the API.

use crate::{Result, Store, epoch, from_epoch};
use serde::Serialize;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StateEvent {
    pub host: String,
    pub service: String,
    #[serde(serialize_with = "rfc3339")]
    pub ts: SystemTime,
    /// `service`, `host` (UP/DOWN from the host check), `flap_start` or
    /// `flap_stop`.
    pub kind: String,
    pub from_state: Option<String>,
    pub to_state: String,
    pub state_type: String,
    pub hard: bool,
    pub output: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Series {
    pub id: i32,
    pub metric: String,
    pub labels: serde_json::Value,
    pub unit: String,
    /// Oldest and newest sample; `None` without samples.
    #[serde(serialize_with = "rfc3339_opt")]
    pub first: Option<SystemTime>,
    #[serde(serialize_with = "rfc3339_opt")]
    pub last: Option<SystemTime>,
}

/// Points of a series, bucketed for display.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Points {
    /// Bucket width in seconds. A bucket holds at most one sample when
    /// this is below the check interval, so the data is effectively raw.
    pub bucket: f64,
    /// `[unix_ms, avg, min, max, count]` per bucket, oldest first.
    pub points: Vec<(f64, f64, f64, f64, i64)>,
}

/// Filters for [`Store::events`].
#[derive(Debug, Clone, Default)]
pub struct EventFilter {
    pub host: Option<String>,
    pub service: Option<String>,
    /// Only events older than this, for paging backwards.
    pub before: Option<SystemTime>,
    /// Only events at or after this.
    pub since: Option<SystemTime>,
    /// Event kinds (see [`StateEvent::kind`]); empty for all.
    pub kinds: Vec<String>,
    /// States changed to, e.g. `critical`, `down`; empty for all.
    pub states: Vec<String>,
    /// `soft` or `hard`.
    pub state_type: Option<String>,
    /// Only hard changes (confirmed problems and their recoveries).
    pub hard_only: bool,
    pub limit: i64,
}

impl Store {
    /// State changes, newest first.
    pub async fn events(&self, filter: &EventFilter) -> Result<Vec<StateEvent>> {
        type Row = (
            String,
            String,
            f64,
            String,
            Option<String>,
            String,
            String,
            bool,
            String,
        );
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT h.name, s.name, extract(epoch FROM e.ts)::float8, e.kind, e.from_state,
                        e.to_state, e.state_type, e.hard, e.output
                 FROM state_events e
                 JOIN services s ON s.id = e.service_id
                 JOIN hosts h ON h.id = s.host_id
                 WHERE ($1::text IS NULL OR h.name = $1)
                   AND ($2::text IS NULL OR s.name = $2)
                   AND ($3::float8 IS NULL OR e.ts < to_timestamp($3))
                   AND ($4::float8 IS NULL OR e.ts >= to_timestamp($4))
                   AND (cardinality($5::text[]) = 0 OR e.kind = ANY($5))
                   AND (cardinality($6::text[]) = 0 OR e.to_state = ANY($6))
                   AND ($7::text IS NULL OR e.state_type = $7)
                   AND (NOT $8 OR e.hard)
                 ORDER BY e.ts DESC
                 LIMIT $9",
        )
        .bind(&filter.host)
        .bind(&filter.service)
        .bind(filter.before.map(epoch))
        .bind(filter.since.map(epoch))
        .bind(&filter.kinds)
        .bind(&filter.states)
        .bind(&filter.state_type)
        .bind(filter.hard_only)
        .bind(filter.limit.clamp(1, 10_000))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(
                |(host, service, ts, kind, from_state, to_state, state_type, hard, output)| {
                    StateEvent {
                        host,
                        service,
                        ts: from_epoch(ts),
                        kind,
                        from_state,
                        to_state,
                        state_type,
                        hard,
                        output,
                    }
                },
            )
            .collect())
    }

    /// The series recorded for a service, `None` if the service is unknown.
    pub async fn series(&self, host: &str, service: &str) -> Result<Option<Vec<Series>>> {
        let service_id: Option<i32> = sqlx::query_scalar(
            "SELECT s.id FROM services s JOIN hosts h ON h.id = s.host_id
             WHERE h.name = $1 AND s.name = $2",
        )
        .bind(host)
        .bind(service)
        .fetch_optional(&self.pool)
        .await?;
        let Some(service_id) = service_id else {
            return Ok(None);
        };
        type Row = (
            i32,
            String,
            serde_json::Value,
            String,
            Option<f64>,
            Option<f64>,
        );
        // min/max per series are index lookups on (series_id, ts).
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT s.id, s.metric, s.labels, s.unit,
                    extract(epoch FROM (SELECT min(ts) FROM samples WHERE series_id = s.id))::float8,
                    extract(epoch FROM (SELECT max(ts) FROM samples WHERE series_id = s.id))::float8
             FROM series s WHERE s.service_id = $1 ORDER BY s.metric, s.labels::text",
        )
        .bind(service_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(Some(
            rows.into_iter()
                .map(|(id, metric, labels, unit, first, last)| Series {
                    id,
                    metric,
                    labels,
                    unit,
                    first: first.map(from_epoch),
                    last: last.map(from_epoch),
                })
                .collect(),
        ))
    }

    /// Samples of a series in `[from, to)`, aggregated into at most about
    /// `max_points` buckets. Masked samples are left out. `None` if the
    /// series does not exist.
    pub async fn points(
        &self,
        series_id: i32,
        from: SystemTime,
        to: SystemTime,
        max_points: u32,
    ) -> Result<Option<Points>> {
        let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM series WHERE id = $1)")
            .bind(series_id)
            .fetch_one(&self.pool)
            .await?;
        if !exists {
            return Ok(None);
        }
        let span = to.duration_since(from).unwrap_or_default();
        let bucket = (span / max_points.max(1)).max(Duration::from_secs(1));
        // date_bin works on plain PostgreSQL (14+) and TimescaleDB alike.
        let points: Vec<(f64, f64, f64, f64, i64)> = sqlx::query_as(
            "SELECT extract(epoch FROM date_bin(make_interval(secs => $4), ts, 'epoch'::timestamptz))::float8 * 1000 AS t,
                    avg(value), min(value), max(value), count(*)
             FROM samples
             WHERE series_id = $1 AND ts >= to_timestamp($2) AND ts < to_timestamp($3)
               AND flags & 4 = 0
             GROUP BY t ORDER BY t",
        )
        .bind(series_id)
        .bind(epoch(from))
        .bind(epoch(to))
        .bind(bucket.as_secs_f64())
        .fetch_all(&self.pool)
        .await?;
        Ok(Some(Points {
            bucket: bucket.as_secs_f64(),
            points,
        }))
    }
}

pub(crate) fn rfc3339<S: serde::Serializer>(
    t: &SystemTime,
    s: S,
) -> std::result::Result<S::Ok, S::Error> {
    s.collect_str(&humantime::format_rfc3339_millis(*t))
}

pub(crate) fn rfc3339_opt<S: serde::Serializer>(
    t: &Option<SystemTime>,
    s: S,
) -> std::result::Result<S::Ok, S::Error> {
    match t {
        Some(t) => rfc3339(t, s),
        None => s.serialize_none(),
    }
}
