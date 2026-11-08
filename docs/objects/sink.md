# sink

A sink copies every check result to a second PostgreSQL database, for
example a central reporting database. Results are first written to a
buffer on disk and then delivered. If the target is down, nothing is lost
until the buffer is full.

```toml
[[sink]]
name = "central"
type = "postgres"
url = "postgres://watcher@central.example.com/watcher"
password_file = "central.pass"
```

Every result is also written to the database `watcher` on
central.example.com, once a minute.

Sinks are written with double brackets, `[[sink]]`, because each one is an
entry in a list.

## Fields

| Field | Default | Description |
|---|---|---|
| `name` | | Required. Unique name of the sink. |
| `type` | | Required. `postgres`. *Not yet:* `hub`. |
| `url` | | Required. The target database: `postgres://user@host[:port]/db` or `postgresql://...`. |
| `password_file` | | File with the database password, relative to this config file. |
| `flush.mode` | `interval` | When results are sent. `interval`: in batches every `flush.interval`. `realtime`: as soon as they arrive. *Not yet:* `adaptive`, `schedule`. |
| `flush.interval` | `1m` | Time between two batches, with `flush.mode = "interval"`. |
| `buffer.max_size` | `1GB` | Size of the disk buffer: `512MB`, `2GB` (factor 1024), or bytes. |
| `buffer.on_full` | `drop_oldest` | What happens when the buffer is full: `drop_oldest` deletes the oldest results. *Not yet:* `downsample`, `block`. |

## Examples

### Every five minutes

```toml
[[sink]]
name = "reporting"
type = "postgres"
url = "postgres://watcher@reporting.example.com/watcher"
password_file = "reporting.pass"
flush = { mode = "interval", interval = "5m" }
```

### As soon as possible

```toml
[[sink]]
name = "reporting"
type = "postgres"
url = "postgres://watcher@reporting.example.com/watcher"
flush.mode = "realtime"
```

### A larger buffer

```toml
[[sink]]
name = "central"
type = "postgres"
url = "postgres://watcher@central.example.com/watcher"
buffer = { max_size = "4GB", on_full = "drop_oldest" }
```

### Written as sub-tables

```toml
[[sink]]
name = "central"
type = "postgres"
url = "postgres://watcher@central.example.com/watcher"

[sink.flush]
mode = "interval"
interval = "1m"

[sink.buffer]
max_size = "1GB"
```

## Behaviour

- The buffer is in `data_dir/sinks/<name>/`.
- Each result is delivered exactly once.
- The target gets the normal Watcher schema. It is created on the first
  connection.
- Several Watcher instances can write to the same target. Host names must
  then be unique across all of them.
- After a failed delivery, Watcher waits longer each time before it tries
  again, up to 5 minutes.
- Sinks are reloaded with the configuration; no restart needed.

*Not yet:* bandwidth limits, compression.

## See also

- [Data](../guides/data.md): the database, SQL views, Grafana
