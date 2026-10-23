# Checks

A check is what to run and how to judge it. Services on hosts use checks
([Hosts and services](hosts-and-services.md)). Fields:
[reference](../reference/objects.md#check).

## Command

```toml
[check.load]
command = "check_load -w 5,4,3 -c 10,8,6"
```

Bare names are looked up in the check directories, then `PATH`:

1. `checks/` next to `watcher.toml`
2. `/usr/lib/monitoring-plugins`, `/usr/lib/nagios/plugins`,
   `/usr/lib64/nagios/plugins`, `/usr/local/libexec/monitoring-plugins`,
   `/usr/local/nagios/libexec` (those that exist)

Own list: `check_dirs` in [watcher.toml](../reference/watcher-toml.md).
New scripts there work without a reload. `watcher config check` warns
about commands not found.

Full path:

```toml
[check.backup_age]
command = "/opt/scripts/backup_age.sh /srv/backup"
```

## Shell

Default: no shell, arguments split like a shell would. Pipes, `$(...)`,
`&&` need `shell = true` (`/bin/sh -c`).

```toml
[check.uptime]
shell = true
command = 'echo "uptime=$(cut -d" " -f1 /proc/uptime)s"'
```

## Macros

| Macro | Value |
|---|---|
| `$name$` | var `name` ([vars](hosts-and-services.md#vars)) |
| `$host.name$`, `$HOSTNAME$` | host name |
| `$host.address$`, `$HOSTADDRESS$` | host address (default: the name) |
| `$host.node$` | host `node` |
| `$host.vars.x$`, `$host.vars.x.y$` | host var `x`, nested `x.y` |
| `$service.name$` | service name |
| `$service.key$` | `for_each` entry key |
| `$check.name$` | check name |

```toml
[check.tcp]
command = "check_tcp -H $host.address$ -p $port$"
vars.port = "22"
```

- Values with spaces stay one argument.
- With `shell = true` values are quoted for the shell: don't put quotes
  around macros.
- Other `$` text (`$HOME`, `$1`, `$(cmd)`) is left alone.
- Unset `$name$`: error for that host.

## Exit code

| Code | State |
|---|---|
| 0 | OK |
| 1 | WARNING |
| 2 | CRITICAL |
| 3 | UNKNOWN |
| other, timeout | UNKNOWN |

## Output formats

Detected automatically. Force with `format = "nagios" | "key_value" |
"json"`.

**Nagios plugin:** text, perfdata after `|`.

```
DISK OK - free space: / 3326 MB (56%) | /=2643MB;5948;5958;0;5968
```

**key=value:** each line only `key=value[unit]` tokens. Labels in
Prometheus style.

```
depth=42
latency=12ms errors=0
temp{sensor="cpu0"}=41
```

**JSON:**

```json
{"state": "warning", "message": "queue filling",
 "metrics": [{"name": "depth", "value": 120, "labels": {"queue": "mail"},
              "warn": "> 100", "crit": "> 500"}]}
```

```json
{"metrics": {"depth": 120, "consumers": 3}}
```

JSON fields: `state` (`ok`, `warning`, `critical`, `unknown`; else the
exit code), `message`, `long_output`, `metrics`. Metric fields: `name`,
`value`, `unit`, `labels`, `warn`, `crit`, `min`, `max`.

Unparsable output: UNKNOWN, no metrics.

## Thresholds

Per metric, on any output format:

```toml
[check.http]
command = "check_http -H $host.address$"
thresholds.time = { warn = "> 500ms", crit = "> 2s" }
```

| Syntax | Alert when |
|---|---|
| `> 100`, `>= 100` | above |
| `< 5`, `<= 5` | below |
| `== 0`, `!= 0` | equal / not equal |
| `10` | outside 0..10 (Nagios range) |
| `10:` | below 10 |
| `~:10` | above 10 |
| `10:20` | outside 10..20 |
| `@10:20` | inside 10..20 |

- Units: `> 500ms`, `>= 10GB`, `< 5%`.
- No unit: milliseconds for time metrics, otherwise the stored unit
  (bytes, percent).
- `_` groups digits: `> 10_000`.
- Metric names with special characters: `thresholds."/" = { crit = "> 50GB" }`.
- Applies to every label set of the metric.

Final state: the worst of exit code and thresholds
(OK < WARNING < UNKNOWN < CRITICAL).

Thresholds in Nagios perfdata (`;5948;5958`) are shown, not evaluated: the
plugin already used them for its exit code. Thresholds in JSON metrics are
evaluated. A configured `warn`/`crit` replaces the embedded one of the same
level.

## Units

| Output | Stored as |
|---|---|
| `s`, `ms`, `us`, `µs`, `ns` | seconds |
| `B`, `KB`, `MB`, `GB`, `TB`, `PB` (×1024), `KiB` … `PiB` | bytes |
| `kB` (×1000) | bytes |
| `%` | percent |
| `c` | counter |
| other (`°C`, `rpm`) | as is |

## Timing

| Field | Default | |
|---|---|---|
| `interval` | `1m` | between checks |
| `retry_interval` | min(interval, 1m) | while SOFT |
| `max_attempts` | 3 (passive: 1) | results until HARD |
| `timeout` | `60s` | then killed, UNKNOWN |

```toml
[check.dns]
command = "check_dns -H example.com -s $host.address$"
interval = "5m"
retry_interval = "30s"
max_attempts = 5
```

Durations: `30s`, `5m`, `1h`, `1day`. Services can set the same fields,
over the check's.

A problem is SOFT 1/3, 2/3, then HARD 3/3. Only HARD changes notify.

On timeout the whole process group is killed. Output is capped at 64 KiB
per stream.

## Flap detection

On by default. Percent state change over the last 21 results: flapping
from 20 %, stops below 5 %. While flapping, only FLAPPINGSTART and
FLAPPINGSTOP notify.

```toml
[check.vpn]
command = "check_ping -H $host.address$ -w 200,20% -c 500,60%"
flap_low = 10
flap_high = 30
```

```toml
[check.batch_job]
command = "/opt/scripts/job_status.sh"
flap_detection = false
```

## Passive checks

Results pushed over the API, e.g. by cron jobs. No command.

```toml
[[host]]
name = "backup01"
  [host.service.backup]
  passive = true
  freshness = "26h"     # UNKNOWN if no result for 26h
```

Push with an API token (Account page, or `watcher token create <user> <name>`):

```sh
curl -X POST https://watcher.example.com/api/v1/services/backup01/backup/result \
  -H "Authorization: Bearer $WATCHER_TOKEN" \
  -H 'content-type: application/json' \
  -d '{"exit_code": 0, "output": "BACKUP OK | size=12GB duration=840s"}'
```

Body: `exit_code` or `state` (`ok`, `warning`, `critical`, `unknown`), and
`output`, parsed like stdout. Thresholds apply.

From a cron job:

```sh
#!/bin/sh
# /etc/cron.daily/backup
if /usr/local/bin/backup.sh; then code=0; msg="BACKUP OK"; else code=2; msg="BACKUP CRITICAL"; fi
curl -fsS -X POST "https://watcher.example.com/api/v1/services/$(hostname)/backup/result" \
  -H "Authorization: Bearer $(cat /etc/watcher-token)" \
  -H 'content-type: application/json' \
  -d "{\"exit_code\": $code, \"output\": \"$msg\"}"
```

## Testing

```sh
watcher run-check -- check_disk -w 20% -c 10% -p /
watcher run-check --shell 'echo "depth=$(mailq | grep -c ^[A-F0-9])"'
watcher run-check --warn 'depth=> 100' --crit 'depth=> 500' -- /opt/scripts/queue.sh
```

```
WARNING - load1=0.5 load5=0.7

  METRIC  VALUE  WARN   CRIT  STATE
  load1   0.5    > 0.1        WARNING
  load5   0.7

exit code 0 · format key_value · took 766.706 µs
```

Exits with the final state (0–3). Options: [CLI](../reference/cli.md#run-check).

## Writing a script

- Print one result: Nagios line, `key=value` lines or JSON.
- Exit 0–3, or let thresholds decide.
- Keep it fast; it runs every `interval`.
- Put it in `checks/` next to `watcher.toml`, make it executable.

`checks/queue_depth`:

```sh
#!/bin/sh
depth=$(find /var/spool/app/queue -type f | wc -l)
echo "depth=$depth"
```

```toml
[check.queue_depth]
command = "queue_depth"
thresholds.depth = { warn = "> 100", crit = "> 500" }
```
