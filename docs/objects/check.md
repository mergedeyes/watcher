# check

A check describes what to run and how to judge the result: a command,
values for its placeholders, thresholds and timing. A check does nothing
by itself. It runs once a [service](service.md) uses it and the service is
on a [host](host.md).

```toml
[check.http]
command = "check_http -H $host.address$ -u $url$"
vars.url = "/"
```

This check runs the `check_http` plugin. `$host.address$` is replaced with
the address of the host it runs for, and `$url$` with the var `url`, which
defaults to `/`. The plugin's exit code gives the state: 0 is OK, 1 WARNING,
2 CRITICAL, 3 UNKNOWN.

A check that no service uses gives a warning.

## Fields

| Field | Default | Description |
|---|---|---|
| `title` | | Name shown in the web UI. Can contain [macros](#macros). |
| `command` | | The command to run. Required, unless `builtin` or `passive` is set. Can contain [macros](#macros). |
| `builtin` | | Use a check built into Watcher instead of a command: `ping`, `tcp`, `http`, `dns` or `tls`. See [Built-in checks](#built-in-checks). |
| `shell` | `false` | `true` runs the command with `/bin/sh -c`, needed for pipes, `$(...)` or `&&`. See [Shell](#shell). |
| `run_on` | `local` | Where the command runs. `local`: on the Watcher machine. `host`: on the monitored host over SSH, see [Checks over SSH](../guides/ssh.md). |
| `copy` | `false` | With `run_on = "host"`: upload the script to the host before running it. See [Copying scripts](../guides/ssh.md#copying-scripts). |
| `passive` | `false` | `true`: Watcher runs nothing and waits for results sent over the API. See [Passive checks](#passive-checks). |
| `format` | `auto` | How the output is read: `auto`, `nagios`, `key_value` or `json`. See [Output formats](#output-formats). |
| `vars` | | Values for the `$name$` macros in the command, or the settings of a [built-in check](#built-in-checks). Text, numbers, or tables. |
| `thresholds.<metric>` | | Limits for a metric: `{ warn = "...", crit = "..." }`. See [Thresholds](#thresholds). |
| `interval` | `1m` | Time between two checks. |
| `retry_interval` | `interval`, at most `1m` | Time between checks while a problem is not yet confirmed (SOFT). |
| `max_attempts` | `3`, passive `1` | How many problem results in a row confirm a problem (HARD). Only HARD problems notify. |
| `timeout` | `60s` | After this time the command is killed and the result is UNKNOWN. |
| `freshness` | | Passive checks only: the state becomes UNKNOWN when no result arrived for this long. |
| `flap_detection` | `true` | Detect services that change state too often. See [Flap detection](#flap-detection). |
| `flap_low` | `5` | Flapping ends when the state change rate drops below this percentage. |
| `flap_high` | `20` | Flapping starts when the state change rate reaches this percentage. |
| `graphs` | `true` | `false`: no graphs for this check's metrics. |

A [service](service.md) or a [host service](host.md#host-services) can set
every one of these fields again, and its value wins over the check's.

`check`, `for_each`, `groups` and `enabled` are not allowed on a check;
they belong on a service.

## Examples

### A plugin with fixed arguments

```toml
[check.load]
command = "check_load -w 5,4,3 -c 10,8,6"
```

### A plugin with vars

The vars give default values. Services and hosts can set other values.

```toml
[check.disk]
command = "check_disk -w $warn$ -c $crit$ -p $path$"
vars = { warn = "20%", crit = "10%", path = "/" }
```

### A script with a full path

```toml
[check.backup_age]
command = "/opt/scripts/backup_age.sh /srv/backup"
```

### A metric threshold

The check fails when the response time of `check_http` is too high.

```toml
[check.http]
command = "check_http -H $host.address$"
thresholds.time = { warn = "> 500ms", crit = "> 2s" }
```

### Checking less often

```toml
[check.dns]
command = "check_dns -H example.com -s $host.address$"
interval = "5m"
retry_interval = "30s"
max_attempts = 5
```

### A pipe in the command

```toml
[check.uptime]
shell = true
command = 'echo "uptime=$(cut -d" " -f1 /proc/uptime)s"'
```

### Running on the host over SSH

```toml
[check.load]
command = "check_load -w 5,4,3 -c 10,8,6"
run_on = "host"
```

The host needs an `ssh` login, see [Checks over SSH](../guides/ssh.md).

### No graphs

```toml
[check.backup]
command = "/opt/scripts/backup.sh"
graphs = false
```

## Commands

A command without a path, like `check_load`, is looked up in these
directories, in this order:

1. `checks/` next to `watcher.toml`
2. the monitoring plugin directories that exist:
   `/usr/lib/monitoring-plugins`, `/usr/lib/nagios/plugins`,
   `/usr/lib64/nagios/plugins`, `/usr/local/libexec/monitoring-plugins`,
   `/usr/local/nagios/libexec`
3. `PATH`

`check_dirs` in [watcher.toml](../reference/watcher-toml.md) replaces the
first two with your own list. New scripts in these directories work
without a reload. `watcher config check` warns about commands it can't
find.

The command is split into arguments the way a shell would do it, but
without a shell: quotes group words, and nothing else is interpreted.

## Shell

By default there is no shell, so pipes (`|`), command substitution
(`$(...)`), `&&` and redirections don't work. With `shell = true` the
command runs with `/bin/sh -c` and all of these work.

```toml
[check.mailq]
shell = true
command = 'echo "depth=$(mailq | grep -c "^[A-F0-9]")"'
```

## Macros

Macros are placeholders in `command` and `title`. They are replaced before
the command runs.

| Macro | Replaced with |
|---|---|
| `$name$` | the var `name`, see [Vars](#vars) |
| `$host.name$` or `$HOSTNAME$` | the host name |
| `$host.address$` or `$HOSTADDRESS$` | the host address; the host name if no address is set |
| `$host.node$` | the host's `node`; `local` for hosts without one |
| `$host.vars.x$` | the host var `x`; nested: `$host.vars.x.y$` |
| `$service.name$` | the service name |
| `$service.key$` | the key of the `for_each` entry, see [for_each](service.md#one-service-per-item-for_each) |
| `$check.name$` | the check name |

```toml
[check.tcp]
command = "check_tcp -H $host.address$ -p $port$"
vars.port = "22"
```

Things to know:

- A value with spaces stays a single argument.
- With `shell = true`, values are quoted for the shell automatically. Don't
  put quotes around macros yourself.
- Any other `$` text, such as `$HOME`, `$1` or `$(cmd)`, is left alone.
- If a `$name$` var is not set anywhere for a host, that service is an
  error on that host.

## Vars

Vars give values to the `$name$` macros. The check sets defaults, and
services, host groups, hosts and host services can set other values:

```toml
[check.http]
command = "check_http -H $host.address$ -u $url$"
vars.url = "/"

[[host]]
name = "shop"
services = ["http"]
vars.url = "/health"          # this host runs: check_http -H shop -u /health
```

Which value wins is explained in
[Which value wins](../guides/configuration.md#which-value-wins).

Built-in checks read their settings from vars too, but not from host and
host group vars. See [Built-in checks](#built-in-checks).

## Exit codes

| Exit code | State |
|---|---|
| 0 | OK |
| 1 | WARNING |
| 2 | CRITICAL |
| 3 | UNKNOWN |
| any other code, or a timeout | UNKNOWN |

When the timeout is reached, the whole process group is killed. Output is
cut off after 64 KiB on stdout and on stderr.

## Output formats

Watcher reads the output of the command and takes metrics from it. The
format is detected automatically; set `format` to force one.

**Nagios plugin** (`nagios`): one line of text, metrics (performance data)
after `|`.

```
DISK OK - free space: / 3326 MB (56%) | /=2643MB;5948;5958;0;5968
```

**key=value** (`key_value`): every line contains only `key=value` pairs,
optionally with a unit. Labels can be added in Prometheus style.

```
depth=42
latency=12ms errors=0
temp{sensor="cpu0"}=41
```

**JSON** (`json`): an object with a state, a message and metrics.

```json
{"state": "warning", "message": "queue filling",
 "metrics": [{"name": "depth", "value": 120, "labels": {"queue": "mail"},
              "warn": "> 100", "crit": "> 500"}]}
```

Metrics can also be a simple object:

```json
{"metrics": {"depth": 120, "consumers": 3}}
```

| JSON field | Description |
|---|---|
| `state` | `ok`, `warning`, `critical` or `unknown`. Without it, the exit code decides. |
| `message` | The text shown as the output. |
| `long_output` | More text, shown on the service page. |
| `metrics` | A list of metric objects, or an object of `name: value`. |

| Metric field | Description |
|---|---|
| `name` | Metric name. |
| `value` | Number. |
| `unit` | Unit, see [Units](#units). |
| `labels` | Object of label names and values. |
| `warn`, `crit` | Thresholds, same syntax as [Thresholds](#thresholds). |
| `min`, `max` | Range, used for graphs. |

Output that can't be read in the chosen format gives UNKNOWN and no
metrics.

## Thresholds

Thresholds compare a metric against limits and raise the state to WARNING
or CRITICAL. They work with every output format.

```toml
[check.http]
command = "check_http -H $host.address$"
thresholds.time = { warn = "> 500ms", crit = "> 2s" }
```

| Syntax | Alert when the value is |
|---|---|
| `> 100`, `>= 100` | above 100 (or equal) |
| `< 5`, `<= 5` | below 5 (or equal) |
| `== 0`, `!= 0` | equal to 0 / not equal to 0 |
| `10` | outside 0 to 10 (Nagios range) |
| `10:` | below 10 |
| `~:10` | above 10 |
| `10:20` | outside 10 to 20 |
| `@10:20` | inside 10 to 20 |

Things to know:

- Limits can have units: `> 500ms`, `>= 10GB`, `< 5%`.
- Without a unit, time metrics are compared in milliseconds, and all other
  metrics in their stored unit (bytes, percent).
- `_` can group digits: `> 10_000`.
- Metric names with special characters need quotes:
  `thresholds."/" = { crit = "> 50GB" }`.
- A threshold applies to every label set of the metric.

The final state is the worst of the exit code and all thresholds. The order
from best to worst is OK, WARNING, UNKNOWN, CRITICAL.

Thresholds that a Nagios plugin prints in its performance data
(`;5948;5958`) are shown in the web UI, but not evaluated again, because
the plugin already used them for its exit code. Thresholds in JSON metrics
are evaluated. A `warn` or `crit` you configure replaces the one in the
output.

## Units

Watcher stores values in base units, so different plugins can be compared
and graphed together.

| Unit in the output | Stored as |
|---|---|
| `s`, `ms`, `us`, `µs`, `ns` | seconds |
| `B`, `KB`, `MB`, `GB`, `TB`, `PB` (factor 1024), `KiB` … `PiB` | bytes |
| `kB` (factor 1000) | bytes |
| `%` | percent |
| `c` | counter |
| anything else (`°C`, `rpm`) | kept as it is |

## SOFT and HARD states

A new problem is not reported at once. Watcher first checks again, every
`retry_interval`, until it has seen the problem `max_attempts` times in a
row. Until then the state is SOFT (shown as `SOFT 1/3`, `SOFT 2/3`); after
that it is HARD (`HARD 3/3`). Only HARD changes send notifications.

A single bad result, like a short network hiccup, therefore doesn't wake
anyone up. Set `max_attempts = 1` to report every problem at once.

## Flap detection

A service flaps when it keeps changing between OK and a problem. Watcher
looks at the last 21 results and computes how often the state changed, as a
percentage. Flapping starts when the rate reaches `flap_high` (default 20 %)
and ends when it drops below `flap_low` (default 5 %).

While a service flaps, its problems and recoveries are not notified. Only
the start and end of flapping are (FLAPPINGSTART, FLAPPINGSTOP).

Less sensitive:

```toml
[check.vpn]
command = "check_ping -H $host.address$ -w 200,20% -c 500,60%"
flap_low = 10
flap_high = 30
```

Off:

```toml
[check.batch_job]
command = "/opt/scripts/job_status.sh"
flap_detection = false
```

## Built-in checks

Built-in checks run inside Watcher and need no plugin. The checks `ping`,
`tcp`, `http`, `dns` and `tls` always exist, even without any
configuration. If you write your own check with one of these names (for
example `[check.http]` with a `check_http` command), it replaces the
built-in one.

You set them up with vars, on the service or on the host's service:

```toml
[service.ping]                 # uses the built-in check "ping"
vars.crit = "500,60%"

[service.ssh]
check = "tcp"
vars = { port = "22", expect = "SSH-" }

[service.web]
check = "http"
vars = { url = "/health", content = "ok", crit = "2s" }

[service.cert]
check = "tls"
vars = { name = "www.example.com" }

[service.dns]                  # on a DNS server: does it answer for our domain?
vars = { name = "example.com", expect = "10.0.0.5" }
```

Built-in checks read the vars of the check, the service and the host
service (`[host.service.<name>]`). Host and host group vars don't apply, so
a host var like `port` that you set for something else can't change them.
To use a host var, write it as a macro: `port = "$host.vars.ssh_port$"`.
Macros work in all of these vars, for example `name = "$host.name$"`.

Every built-in check also takes `address`: it then connects there instead
of the host's address (for `dns`, that is the server it asks). This is
useful for hosts that stand for several sites, see
[Many websites](../examples.md#many-websites).

`warn` and `crit` of `tcp`, `http` and `dns` are response times: seconds
(`0.5`) or a duration (`500ms`, `2s`). Without them, the response time
doesn't change the state. A check also ends shortly before its `timeout`
and reports CRITICAL ("no answer within ...") instead of being stopped.

The metrics of built-in checks work like a plugin's: you can set
[thresholds](#thresholds) on them (`thresholds.time = { crit = "> 2s" }`)
and see them in graphs.

### ping

ICMP echo, like `check_ping`.

| Var | Default | Description |
|---|---|---|
| `packets` | `5` | Number of ICMP echo requests, sent 200 ms apart. |
| `warn` | `3000,80%` | WARNING at this average round trip time (ms) or packet loss. |
| `crit` | `5000,100%` | CRITICAL at this average round trip time (ms) or packet loss. |

The result looks like the one of `check_ping`, with the metrics `rta`
(round trip average, ms) and `pl` (packet loss, %). DNS names are resolved
on every run; a name that can't be resolved gives CRITICAL.

Sending ICMP needs permission. Without it, the result is UNKNOWN. Most
distributions allow it by default; otherwise allow it with
`sysctl net.ipv4.ping_group_range="0 2147483647"` or give Watcher
`CAP_NET_RAW`.

### tcp

Connects to a port of the host, like `check_tcp`. Optionally sends
something and checks the answer.

| Var | Default | Description |
|---|---|---|
| `port` | | The port. Required. |
| `send` | | Text to send after connecting. `\r`, `\n`, `\t` and `\\` are written as such. |
| `expect` | | Text the answer must contain, for example `SSH-` or `220`. Without it, nothing is read. |
| `warn` | | WARNING at this response time. |
| `crit` | | CRITICAL at this response time. |

A refused connection or a missing `expect` text gives CRITICAL. Watcher
reads the answer until `expect` shows up, the server closes the
connection, or nothing more comes for a second. Metric: `time` (s).

```
TCP OK - 10.0.0.5:22 answered in 1.3 ms: SSH-2.0-OpenSSH_9.6
```

### http

Requests a URL, like `check_http`.

| Var | Default | Description |
|---|---|---|
| `url` | `/` | A full URL (`https://www.example.com/login`), or a path on the host's address (`/health`). |
| `method` | `GET` | `GET`, `HEAD`, `POST`, `PUT`, ... |
| `headers` | | Request headers, as a table: `headers = { Accept = "application/json" }`. A `Host` header asks for a different site on the same address; `User-Agent` replaces Watcher's. |
| `body` | | Text to send as the request body, for example JSON for a `POST`. Set the matching `Content-Type` in `headers`. |
| `user` | | Log in with HTTP basic auth as this user. |
| `password` | | The password for `user`. |
| `status` | | Status codes that are OK, for example `200` or `200,301-302`. Others give CRITICAL. Without it: 2xx and 3xx are OK, 4xx WARNING, 5xx CRITICAL. |
| `content` | | Text the body must contain. Only the first MiB of the body is searched. |
| `follow` | `false` | Follow redirects (up to 10). Without it, a redirect is the answer. |
| `verify` | `true` | Check the server's certificate. `false` accepts any certificate. |
| `warn` | | WARNING at this response time. |
| `crit` | | CRITICAL at this response time. |

Certificates are checked against the system's trust store, so internal
CAs that the system trusts work. Proxy settings from the environment
(`HTTP_PROXY`) are not used. Metrics: `time` (s) and `size` (bytes).

An API that needs a token and a JSON request:

```toml
[service.api]
check = "http"
vars.url = "https://api.example.com/v1/status"
vars.method = "POST"
vars.body = '{"probe": true}'
vars.headers = { Content-Type = "application/json", Authorization = "Bearer $api_token$" }
vars.api_token = "abc123"
vars.content = '"ok":true'
```

Headers from the check, the service and the host service are merged by
name, so a check can set `Accept` and a service add its own token. The
command shown in the web UI leaves out the password and the values of
headers that look like credentials (names with `auth`, `token`, `key`,
`cookie`, ...). They are still readable in the config files and the
Config Tree.

```
HTTP OK - HTTP/2.0 200 OK, 577 bytes in 87 ms
```

### dns

Looks up a name at a DNS server, like `check_dns`. By default the
server is the host itself.

| Var | Default | Description |
|---|---|---|
| `name` | | The name to look up. Required. |
| `type` | `A` | `A`, `AAAA`, `CNAME`, `MX`, `NS`, `PTR`, `SOA` or `TXT`. |
| `server` | the host's address | The server to ask. `system` uses the servers in `/etc/resolv.conf`. |
| `port` | `53` | The server's port. |
| `expect` | | Values that must all be in the answer, comma-separated: `10.0.0.5, 10.0.0.6`. For MX answers, the mail server's name is enough. |
| `warn` | | WARNING at this response time. |
| `crit` | | CRITICAL at this response time. |

A name that doesn't exist, an empty answer, or a missing `expect` value
gives CRITICAL. The answer is never cached. Metric: `time` (s).

```
DNS OK - example.com A 10.0.0.5 in 2.1 ms (10.0.0.53)
```

### tls

Connects with TLS and checks the server's certificate: whether it is
trusted, whether it is valid for the name, and how many days are left
until it expires. Like `check_ssl_cert`.

| Var | Default | Description |
|---|---|---|
| `port` | `443` | The port. |
| `name` | the host's address | The name the certificate must be valid for. It is also sent to the server (SNI), so servers with several certificates show the right one. |
| `warn` | `30` | WARNING at this many days before expiry. |
| `crit` | `14` | CRITICAL at this many days before expiry. |
| `verify` | `true` | An untrusted certificate, or one not valid for `name`, gives CRITICAL. `false` only checks the dates. |

Most certificates are issued for names, not addresses. If the host's
address is an IP address, set `name`. An expired certificate gives
CRITICAL. Certificates are checked against the system's trust store.
Metric: `days`. Only TLS from the start is supported; STARTTLS (SMTP,
IMAP) isn't yet.

```
TLS OK - www.example.com: valid until 2027-02-02 (85 days), issued by R11
```

### Under another name

A built-in check under another name, with other defaults:

```toml
[check.ping_fast]
builtin = "ping"
vars = { packets = "2", warn = "100,20%", crit = "500,60%" }

[check.https]
builtin = "http"
vars = { url = "https://$host.name$/", status = "200" }
```

Built-in checks always run on the Watcher machine, never over SSH.

## Passive checks

A passive check runs nothing. Results are sent to Watcher over the API, for
example by a cron job at the end of a backup.

```toml
[check.backup]
passive = true
freshness = "26h"     # UNKNOWN if no result arrived for 26 hours
```

Passive checks confirm a problem with the first result (`max_attempts`
defaults to 1), because the next result may be a day away.

Send a result with an API token (from the Account page, or
`watcher token create <user> <name>`):

```sh
curl -X POST https://watcher.example.com/api/v1/services/backup01/backup/result \
  -H "Authorization: Bearer $WATCHER_TOKEN" \
  -H 'content-type: application/json' \
  -d '{"exit_code": 0, "output": "BACKUP OK | size=12GB duration=840s"}'
```

The body contains `exit_code` (0 to 3) or `state` (`ok`, `warning`,
`critical`, `unknown`), and `output`. The output is read like the output of
a command, so metrics and thresholds work.

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

## Testing a command

`watcher run-check` runs a command once and shows how Watcher reads the
result, without any configuration:

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

The exit code of `run-check` is the final state (0 to 3). All options:
[run-check](../reference/cli.md#run-check).

In the web UI, **Run Check** on a service page runs that service's command
once, the same way the scheduler would.

## Writing your own script

- Print one result: a Nagios line, `key=value` lines, or JSON.
- Exit with 0 to 3, or exit with 0 and let thresholds decide.
- Keep it fast; it runs every `interval`.
- Put it into `checks/` next to `watcher.toml` and make it executable.
  Then you can use it by name, without a path.

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

## See also

- [service](service.md): put a check on hosts
- [Checks over SSH](../guides/ssh.md)
- [Which value wins](../guides/configuration.md#which-value-wins)
