use crate::sync::state_type_str;
use crate::{Record, Result, Store, epoch};
use std::collections::HashMap;
use watcher_core::State;

/// Identity of a series: service id, metric name, labels as JSON.
pub(crate) type SeriesKey = (i32, String, String);

/// Series ids resolved so far, kept across writes. Only extended after a
/// transaction commits.
#[derive(Debug, Default)]
pub struct SeriesCache(HashMap<SeriesKey, i32>);

impl Store {
    /// Writes `records` in one transaction. Records of services without an
    /// id in `ids` (removed by a reload) are skipped.
    pub(crate) async fn write(
        &self,
        records: &[Record],
        ids: &HashMap<String, i32>,
        series: &mut SeriesCache,
    ) -> Result<()> {
        let records: Vec<(i32, &Record)> = records
            .iter()
            .filter_map(|r| ids.get(&r.service).map(|&id| (id, r)))
            .collect();
        if records.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;
        let resolved = write_tx(&mut tx, &records, &series.0).await?;
        tx.commit().await?;
        series.0.extend(resolved);
        Ok(())
    }

    /// Writes a batch from a sink exactly once: `source` identifies the
    /// sending instance and sink, and records with a sequence number at or
    /// below the last one applied for it are skipped. Progress is stored in
    /// the same transaction, so a retried batch never duplicates data.
    pub async fn write_sink(
        &self,
        source: &str,
        batch: &[(u64, Record)],
        ids: &HashMap<String, i32>,
        series: &mut SeriesCache,
    ) -> Result<()> {
        let Some(last) = batch.iter().map(|(seq, _)| *seq).max() else {
            return Ok(());
        };
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO sink_progress (source, seq) VALUES ($1, 0) ON CONFLICT DO NOTHING",
        )
        .bind(source)
        .execute(&mut *tx)
        .await?;
        let done: i64 =
            sqlx::query_scalar("SELECT seq FROM sink_progress WHERE source = $1 FOR UPDATE")
                .bind(source)
                .fetch_one(&mut *tx)
                .await?;
        let fresh: Vec<(i32, &Record)> = batch
            .iter()
            .filter(|(seq, _)| *seq as i64 > done)
            .filter_map(|(_, r)| ids.get(&r.service).map(|&id| (id, r)))
            .collect();
        let resolved = if fresh.is_empty() {
            HashMap::new()
        } else {
            write_tx(&mut tx, &fresh, &series.0).await?
        };
        sqlx::query(
            "UPDATE sink_progress SET seq = GREATEST(seq, $2), updated = now() WHERE source = $1",
        )
        .bind(source)
        .bind(last as i64)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        series.0.extend(resolved);
        Ok(())
    }
}

/// Writes samples, current state and state changes of `records`. Returns
/// the series ids it had to create.
async fn write_tx(
    conn: &mut sqlx::PgConnection,
    records: &[(i32, &Record)],
    series: &HashMap<SeriesKey, i32>,
) -> Result<HashMap<SeriesKey, i32>> {
    // Samples, resolving series ids not in the cache first.
    let mut samples = Vec::new();
    let mut missing: HashMap<SeriesKey, &str> = HashMap::new();
    for &(service_id, record) in records {
        let ts = epoch(record.state.last_check);
        for m in &record.result.metrics {
            let m = &m.metric;
            if !m.value.is_finite() {
                continue;
            }
            let labels = serde_json::to_string(&m.labels).unwrap_or_else(|_| "{}".into());
            let key = (service_id, clean(&m.name), clean(&labels));
            if !series.contains_key(&key) {
                missing.insert(key.clone(), m.unit.symbol());
            }
            samples.push((key, ts, m.value));
        }
    }
    let resolved = resolve_series(&mut *conn, missing).await?;
    let lookup = |key: &SeriesKey| series.get(key).or_else(|| resolved.get(key)).copied();
    let (mut sample_ids, mut sample_ts, mut sample_values) = (Vec::new(), Vec::new(), Vec::new());
    for (key, ts, value) in &samples {
        if let Some(id) = lookup(key) {
            sample_ids.push(id);
            sample_ts.push(*ts);
            sample_values.push(*value);
        }
    }
    sqlx::query(
        "INSERT INTO samples (series_id, ts, value)
         SELECT s, to_timestamp(t), v FROM UNNEST($1::int[], $2::float8[], $3::float8[]) AS u(s, t, v)",
    )
    .bind(&sample_ids)
    .bind(&sample_ts)
    .bind(&sample_values)
    .execute(&mut *conn)
    .await?;

    // Current state: only the latest record per service, since one
    // upsert statement cannot touch a row twice.
    let mut latest: HashMap<i32, &Record> = HashMap::new();
    for &(id, record) in records {
        latest.insert(id, record);
    }
    let mut st = StateRows::default();
    for (&id, record) in &latest {
        st.push(id, record);
    }
    sqlx::query(
        "INSERT INTO service_state (service_id, state, state_type, attempt, last_check,
                                    last_change, last_hard_change, output, long_output, duration,
                                    history, history_len, flapping)
         SELECT u.id, u.state, u.state_type, u.attempt, to_timestamp(u.last_check),
                to_timestamp(u.last_change), to_timestamp(u.last_hard_change),
                u.output, u.long_output, u.duration, u.history, u.history_len, u.flapping
         FROM UNNEST($1::int[], $2::text[], $3::text[], $4::int[], $5::float8[],
                     $6::float8[], $7::float8[], $8::text[], $9::text[], $10::float8[],
                     $11::int8[], $12::int2[], $13::bool[])
              AS u(id, state, state_type, attempt, last_check, last_change,
                   last_hard_change, output, long_output, duration, history, history_len, flapping)
         ON CONFLICT (service_id) DO UPDATE SET
             state = EXCLUDED.state, state_type = EXCLUDED.state_type,
             attempt = EXCLUDED.attempt, last_check = EXCLUDED.last_check,
             last_change = EXCLUDED.last_change, last_hard_change = EXCLUDED.last_hard_change,
             output = EXCLUDED.output, long_output = EXCLUDED.long_output,
             duration = EXCLUDED.duration, history = EXCLUDED.history,
             history_len = EXCLUDED.history_len, flapping = EXCLUDED.flapping",
    )
    .bind(&st.ids)
    .bind(&st.states)
    .bind(&st.types)
    .bind(&st.attempts)
    .bind(&st.last_check)
    .bind(&st.last_change)
    .bind(&st.last_hard_change)
    .bind(&st.output)
    .bind(&st.long_output)
    .bind(&st.duration)
    .bind(&st.history)
    .bind(&st.history_len)
    .bind(&st.flapping)
    .execute(&mut *conn)
    .await?;

    // State changes, in order.
    let changes: Vec<_> = records
        .iter()
        .filter_map(|&(id, r)| r.change.map(|c| (id, r, c)))
        .collect();
    if !changes.is_empty() {
        let ids: Vec<i32> = changes.iter().map(|c| c.0).collect();
        let ts: Vec<f64> = changes
            .iter()
            .map(|c| epoch(c.1.state.last_check))
            .collect();
        let from: Vec<Option<&str>> = changes
            .iter()
            .map(|c| c.2.from.map(|f| state_str(f.0)))
            .collect();
        let to: Vec<&str> = changes.iter().map(|c| state_str(c.2.to.0)).collect();
        let types: Vec<&str> = changes.iter().map(|c| state_type_str(c.2.to.1)).collect();
        let hard: Vec<bool> = changes.iter().map(|c| c.2.hard).collect();
        let output: Vec<String> = changes.iter().map(|c| clean(&c.1.result.message)).collect();
        sqlx::query(
            "INSERT INTO state_events (service_id, ts, from_state, to_state, state_type, hard, output)
             SELECT u.id, to_timestamp(u.ts), u.from_state, u.to_state, u.state_type, u.hard, u.output
             FROM UNNEST($1::int[], $2::float8[], $3::text[], $4::text[], $5::text[], $6::bool[], $7::text[])
                  AS u(id, ts, from_state, to_state, state_type, hard, output)",
        )
        .bind(&ids)
        .bind(&ts)
        .bind(&from)
        .bind(&to)
        .bind(&types)
        .bind(&hard)
        .bind(&output)
        .execute(&mut *conn)
        .await?;
    }

    Ok(resolved)
}

/// Inserts series that don't exist yet and returns the ids of all `missing`.
async fn resolve_series(
    tx: &mut sqlx::PgConnection,
    missing: HashMap<SeriesKey, &str>,
) -> Result<HashMap<SeriesKey, i32>> {
    if missing.is_empty() {
        return Ok(HashMap::new());
    }
    let keys: Vec<(SeriesKey, &str)> = missing.into_iter().collect();
    let service_ids: Vec<i32> = keys.iter().map(|k| k.0.0).collect();
    let metrics: Vec<&str> = keys.iter().map(|k| k.0.1.as_str()).collect();
    let labels: Vec<&str> = keys.iter().map(|k| k.0.2.as_str()).collect();
    let units: Vec<&str> = keys.iter().map(|k| k.1).collect();
    // The plain SELECT does not see rows inserted by the CTE in the same
    // statement, so both sides are joined.
    let rows: Vec<(i64, i32)> = sqlx::query_as(
        "WITH input AS (
             SELECT * FROM UNNEST($1::int[], $2::text[], $3::text[], $4::text[])
                      WITH ORDINALITY AS u(service_id, metric, labels, unit, n)
         ), ins AS (
             INSERT INTO series (service_id, metric, labels, unit)
             SELECT service_id, metric, labels::jsonb, unit FROM input
             ON CONFLICT (service_id, metric, labels) DO NOTHING
             RETURNING id, service_id, metric, labels
         )
         SELECT i.n, COALESCE(ins.id, s.id)
         FROM input i
         LEFT JOIN ins ON ins.service_id = i.service_id AND ins.metric = i.metric
                      AND ins.labels = i.labels::jsonb
         LEFT JOIN series s ON s.service_id = i.service_id AND s.metric = i.metric
                           AND s.labels = i.labels::jsonb",
    )
    .bind(&service_ids)
    .bind(&metrics)
    .bind(&labels)
    .bind(&units)
    .fetch_all(&mut *tx)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(n, id)| (keys[n as usize - 1].0.clone(), id))
        .collect())
}

#[derive(Default)]
struct StateRows {
    ids: Vec<i32>,
    states: Vec<&'static str>,
    types: Vec<&'static str>,
    attempts: Vec<i32>,
    last_check: Vec<f64>,
    last_change: Vec<f64>,
    last_hard_change: Vec<f64>,
    output: Vec<String>,
    long_output: Vec<String>,
    duration: Vec<f64>,
    history: Vec<i64>,
    history_len: Vec<i16>,
    flapping: Vec<bool>,
}

impl StateRows {
    fn push(&mut self, id: i32, r: &Record) {
        self.ids.push(id);
        self.states.push(state_str(r.state.state));
        self.types.push(state_type_str(r.state.state_type));
        self.attempts
            .push(r.state.attempt.min(i32::MAX as u32) as i32);
        self.last_check.push(epoch(r.state.last_check));
        self.last_change.push(epoch(r.state.last_change));
        self.last_hard_change.push(epoch(r.state.last_hard_change));
        self.output.push(clean(&r.result.message));
        self.long_output.push(clean(&r.result.long_output));
        self.duration.push(r.duration.as_secs_f64());
        // 42 bits: fits.
        self.history.push(r.state.history.bits as i64);
        self.history_len.push(i16::from(r.state.history.len));
        self.flapping.push(r.state.flapping);
    }
}

fn state_str(s: State) -> &'static str {
    match s {
        State::Ok => "ok",
        State::Warning => "warning",
        State::Critical => "critical",
        State::Unknown => "unknown",
    }
}

/// PostgreSQL text cannot hold NUL bytes, which plugin output may contain.
fn clean(s: &str) -> String {
    s.replace('\0', "\u{FFFD}")
}
