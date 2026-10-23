# Data

Results go to PostgreSQL: states, alert history, metrics. TimescaleDB
optional.

## Database

```toml
[database]
url = "postgres://watcher:secret@localhost/watcher"
```

Or `--database URL`, or `WATCHER_DATABASE_URL`. Tables are created and
migrated on start.

| With TimescaleDB | Plain PostgreSQL |
|---|---|
| `samples` hypertable, daily chunks, compression after 2 days (Timescale License build) | plain table |

Without a database: current states in memory only, no history, graphs or
logins.

Values are stored normalized: seconds, bytes, percent
([units](checks.md#units)).

## Tables and views

| | |
|---|---|
| `v_metrics` | tenant, host, service, metric, labels, unit, ts, value, flags |
| `v_service_state` | tenant, host, service, state, state_type, attempt, last_check, last_change, last_hard_change, output, flapping |
| `hosts` | name, address, tags, vars, removed_at |
| `services` | host_id, name, check_hash, removed_at |
| `series` | service_id, metric, labels, unit |
| `samples` | series_id, ts, value, flags |
| `state_events` | service_id, ts, kind, from_state, to_state, state_type, hard, output |
| `check_changes` | service_id, ts, check_hash: every change of a service's command |

- `state_events.kind`: `service`, `host` (UP/DOWN), `flap_start`,
  `flap_stop`.
- `samples.flags`: 1 SYNTHETIC, 2 SUSPECT, 4 MASKED. `v_metrics` hides
  MASKED.
- Hosts and services removed from the config get `removed_at`; history
  stays.

## Read-only login

For Grafana, BI tools, SQL. Sees monitoring tables and views, not users,
sessions or tokens.

```sh
watcher db reader grafana                    # generates and prints a password
echo 'secret' | watcher db reader grafana --password-stdin
```

Needs a database user with CREATEROLE. Rerun after upgrades.

## SQL examples

Last hour of a metric:

```sql
SELECT ts, value FROM v_metrics
WHERE host = 'web01' AND service = 'http' AND metric = 'time'
  AND ts > now() - interval '1 hour'
ORDER BY ts;
```

Current problems:

```sql
SELECT host, service, state, output FROM v_service_state
WHERE state <> 'ok' AND state_type = 'hard';
```

Hourly average:

```sql
SELECT date_bin('1 hour', ts, '2000-01-01') AS hour, avg(value)
FROM v_metrics
WHERE host = 'db01' AND service = 'disk_data' AND metric = '/srv'
GROUP BY hour ORDER BY hour;
```

Hard problems per host, last 30 days:

```sql
SELECT h.name, count(*)
FROM state_events e
JOIN services s ON s.id = e.service_id
JOIN hosts h ON h.id = s.host_id
WHERE e.hard AND e.to_state <> 'ok' AND e.ts > now() - interval '30 days'
GROUP BY h.name ORDER BY count DESC;
```

## Grafana

Data source PostgreSQL, user `grafana` from `watcher db reader`. Panel
query:

```sql
SELECT ts AS time, value, host
FROM v_metrics
WHERE service = 'load' AND metric = 'load1' AND $__timeFilter(ts)
ORDER BY ts
```

## Over the API

```sh
curl -H "Authorization: Bearer $T" https://watcher.example.com/api/v1/services/web01/http/series
curl -H "Authorization: Bearer $T" 'https://watcher.example.com/api/v1/series/42/points?from=6h'
```

[API reference](../reference/api.md).

## Sinks

Copy every result to another PostgreSQL, e.g. a central reporting
database. Buffered on disk (`data_dir/sinks/<name>/`) until delivered:
an unreachable target loses nothing up to `buffer.max_size`. Exactly-once
delivery.

```toml
# conf.d/sinks.toml
[[sink]]
name = "central"
type = "postgres"
url = "postgres://watcher@central.example.com/watcher"
password_file = "central.pass"          # relative to this file

[sink.flush]
mode = "interval"                       # interval | realtime
interval = "1m"

[sink.buffer]
max_size = "1GB"
on_full = "drop_oldest"
```

Realtime:

```toml
[[sink]]
name = "reporting"
type = "postgres"
url = "postgres://watcher@reporting.example.com/watcher"
flush.mode = "realtime"
```

- The target gets the normal schema, created on first connect.
- Several instances may share one target; host names must be unique
  across them.
- Failures back off up to 5 minutes.
- Sinks reload with the config.

Fields: [reference](../reference/objects.md#sink).

*Not yet:* `type = "hub"`, adaptive and scheduled flush, bandwidth limits,
compression, `on_full = "downsample"`.

## Data repair

*Not yet* (Phase 3): select a range in a graph and delete, interpolate,
replace; undo; rollups and retention.
