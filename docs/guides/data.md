# Data

Watcher stores results in PostgreSQL: current states, the alert history and
all metrics. TimescaleDB is optional and makes metric storage smaller and
faster. Everything is in plain tables and views, so you can query it with
SQL, Grafana or any BI tool.

## Database

```toml
# watcher.toml
[database]
url = "postgres://watcher:secret@localhost/watcher"
```

The URL can also be given with `--database URL` or the environment variable
`WATCHER_DATABASE_URL`. Watcher creates and updates its tables on start.

| With TimescaleDB | With plain PostgreSQL |
|---|---|
| `samples` is a hypertable with daily chunks, compressed after 2 days (needs the Timescale License build) | `samples` is a normal table |

Without a database, Watcher keeps only the current states, in memory. There
is no history, no graphs and no login.

Values are stored in base units: seconds, bytes and percent. See
[Units](../objects/check.md#units).

## Tables and views

| Name | Columns |
|---|---|
| `v_metrics` | tenant, host, service, metric, labels, unit, ts, value, flags |
| `v_service_state` | tenant, host, service, state, state_type, attempt, last_check, last_change, last_hard_change, output, flapping |
| `hosts` | name, address, groups (host groups), vars, removed_at |
| `services` | host_id, name, check_hash, removed_at |
| `series` | service_id, metric, labels, unit |
| `samples` | series_id, ts, value, flags |
| `state_events` | service_id, ts, kind, from_state, to_state, state_type, hard, output |
| `check_changes` | service_id, ts, check_hash: every change of a service's command |

- `state_events.kind` is `service`, `host` (UP/DOWN), `flap_start` or
  `flap_stop`.
- `v_service_state` gets a state change at once. Without a change,
  `last_check` and `output` are updated every 30 seconds.
- `samples.flags`: 1 = synthetic, 2 = suspect, 4 = masked. `v_metrics`
  hides masked values.
- Hosts and services removed from the configuration get a `removed_at`
  time. Their history stays.

## Read-only login

For Grafana, BI tools and SQL, create a database user that can only read.
It sees the monitoring tables and views, but not users, sessions or tokens.

```sh
watcher db reader grafana                                  # generates and prints a password
echo 'secret' | watcher db reader grafana --password-stdin # sets your own password
```

Watcher's database user needs the CREATEROLE right for this. Run the
command again after upgrades, so the reader can see new tables.

## SQL examples

The last hour of a metric:

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

Hourly averages:

```sql
SELECT date_bin('1 hour', ts, '2000-01-01') AS hour, avg(value)
FROM v_metrics
WHERE host = 'db01' AND service = 'disk_data' AND metric = '/srv'
GROUP BY hour ORDER BY hour;
```

Confirmed problems per host in the last 30 days:

```sql
SELECT h.name, count(*)
FROM state_events e
JOIN services s ON s.id = e.service_id
JOIN hosts h ON h.id = s.host_id
WHERE e.hard AND e.to_state <> 'ok' AND e.ts > now() - interval '30 days'
GROUP BY h.name ORDER BY count DESC;
```

## Grafana

Add a PostgreSQL data source with the user from `watcher db reader`. A
panel query:

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

See the [API reference](../reference/api.md#history-and-metrics).

## Copying to another database

A [sink](../objects/sink.md) copies every result to a second PostgreSQL
database, for example a central reporting database, with a buffer on disk
so nothing is lost while the target is down.

## Data repair

*Not yet* (Phase 3): select a range in a graph and delete, interpolate or
replace the values, with undo; rollups and retention.
