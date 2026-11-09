# service

A service is a check with its settings, defined once and put on as many
hosts as you like. On a host, a service has the ID `host/service`, for
example `web01/disk_root`.

```toml
[service.http]

[service.disk_root]
check = "disk"
vars.path = "/"
interval = "5m"
```

`[service.http]` uses the check `http`, because a service without `check`
uses the check with the same name. `disk_root` uses the check `disk` and
sets its own `path` and `interval`.

A service runs only on hosts that list it, directly or through a group.
See [How a host gets its services](../guides/configuration.md#how-a-host-gets-its-services).
A service that is on no host gives a warning.

## Fields

| Field | Default | Description |
|---|---|---|
| `check` | the check with the same name | The [check](check.md) to use. Not needed if the service has its own `command`, or is `passive`. |
| `for_each` | | Name of a host var. Creates one service per entry of that var. See [for_each](#one-service-per-item-for_each). |
| `groups` | | Service groups this service belongs to. See [servicegroup](servicegroup.md). |
| `enabled` | `true` | `false`: the service is not created on any host. |
| `title` | the check's `title` | Name shown in the web UI. Can contain [macros](check.md#macros). |
| `command` | the check's | The command to run. Can contain [macros](check.md#macros). |
| `builtin` | the check's | Use a check built into Watcher instead of a command: `ping`, `tcp`, `http`, `dns` or `tls`. See [Built-in checks](check.md#built-in-checks). |
| `shell` | `false` | `true` runs the command with `/bin/sh -c`, needed for pipes, `$(...)` or `&&`. |
| `run_on` | `local` | Where the command runs. `local`: on the Watcher machine. `host`: on the monitored host over SSH, see [Checks over SSH](../guides/ssh.md). |
| `copy` | `false` | With `run_on = "host"`: upload the script to the host before running it. |
| `passive` | `false` | `true`: Watcher runs nothing and waits for results sent over the API. See [Passive checks](check.md#passive-checks). |
| `format` | `auto` | How the output is read: `auto`, `nagios`, `key_value` or `json`. See [Output formats](check.md#output-formats). |
| `vars` | | Values for the `$name$` macros in the command, or the settings of a [built-in check](check.md#built-in-checks). Merged with the check's vars. |
| `thresholds.<metric>` | | Limits for a metric: `{ warn = "...", crit = "..." }`. See [Thresholds](check.md#thresholds). |
| `interval` | `1m` | Time between two checks. |
| `retry_interval` | `interval`, at most `1m` | Time between checks while a problem is not yet confirmed (SOFT). |
| `max_attempts` | `3`, passive `1` | How many problem results in a row confirm a problem (HARD). Only HARD problems notify. |
| `timeout` | `60s` | After this time the command is killed and the result is UNKNOWN. |
| `freshness` | | Passive services only: the state becomes UNKNOWN when no result arrived for this long. |
| `flap_detection` | `true` | Detect services that change state too often. See [Flap detection](check.md#flap-detection). |
| `flap_low` | `5` | Flapping ends when the state change rate drops below this percentage. |
| `flap_high` | `20` | Flapping starts when the state change rate reaches this percentage. |
| `graphs` | `true` | `false`: no graphs for this service's metrics. |

Every field from `title` down can also be set on the check. A value set on
the service wins over the check's value. A
[host service](host.md#host-services) can set every field again for one
host.

## Examples

### Use a check as it is

```toml
[service.ping]
```

### Use a check under another name

```toml
[service.disk_root]
check = "disk"
vars.path = "/"
```

### Change the check's settings

```toml
[service.http_slow]
check = "http"
interval = "5m"
thresholds.time = { warn = "> 2s", crit = "> 10s" }
```

### Without a separate check

A service can have its own `command`. Then it needs no check.

```toml
[service.uptime]
command = "check_uptime"
```

### A passive service

```toml
[service.backup]
passive = true
freshness = "26h"
```

### In a service group

```toml
[service.http]
groups = ["webshop"]
```

### Turned off everywhere

```toml
[service.legacy_app]
enabled = false
```

A host service with `enabled = true` turns it back on for one host. To turn
a service off on one host only, see
[Removing a service from one host](host.md#removing-a-service-from-one-host).

### Put on hosts

A service runs once a host or host group lists it:

```toml
[hostgroup.linux]
members = ["*"]
services = ["ping", "disk_root"]
```

## One service per item: for_each

Some things exist several times on a host: disks, websites, queues. Instead
of writing one service per disk, write one service with `for_each`.
Watcher then creates one service for every entry of a host var.

```toml
[check.disk]
command = "check_disk -w $warn$ -c $crit$ -p $path$"
vars = { warn = "20%", crit = "10%" }

[service.disk]
for_each = "disks"

[[host]]
name = "db01"
services = ["disk"]
vars.disks.root = { path = "/" }
vars.disks.data = { path = "/srv", warn = "10%", crit = "5%" }
```

`vars.disks` on the host has two entries, `root` and `data`. So db01 gets
two services, named `<service>_<key>`:

```
db01/disk_data  every 1m     check_disk -w '10%' -c '5%' -p /srv
db01/disk_root  every 1m     check_disk -w '20%' -c '10%' -p /
```

The values in each entry become vars of that service. `data` sets its own
`warn` and `crit`; `root` uses the check's.

Things to know:

- The host var must be a table of tables, like `vars.disks.root = { ... }`.
- An entry can be empty (`vars.disks.root = {}`) if it needs no vars.
- The entry's key is available as the macro `$service.key$`.
- The var can come from host groups as well as from the host. Entries from
  both add up.
- For a nested var, write the path with dots: `for_each = "site.tld"`.
- To change one of the created services on a host, use its full name:
  `[host.service.disk_root]`. To change all of them, use the service name:
  `[host.service.disk]`.

### Entries from a host group

```toml
[hostgroup.linux]
members = ["db*"]
services = ["disk"]
vars.disks.root = { path = "/" }        # every member gets disk_root

[[host]]
name = "db01"
vars.disks.data = { path = "/srv" }     # db01 also gets disk_data
```

### Titles

A title without macros gets the entry key added: `title = "DISK"` shows as
"DISK (data)". A title with macros is used as it is written:

```toml
[service.cert]
title = "CERT ($website$)"
for_each = "sites"
```

```
CERT (klarsocial.eu)
```

### Using the key in the command

```toml
[check.https]
command = "check_http -S -H $host.vars.site.name$.$service.key$"

[service.https]
for_each = "site.tld"

[[host]]
name = "web01"
services = ["https"]
vars.site.name = "klarsocial"
vars.site.tld.eu = {}
vars.site.tld.de = {}
```

```
web01/https_de  every 1m     check_http -S -H klarsocial.de
web01/https_eu  every 1m     check_http -S -H klarsocial.eu
```

## See also

- [check](check.md): the command and how its output is read
- [host](host.md): change a service on one host
- [servicegroup](servicegroup.md): assign several services together
- [Which value wins](../guides/configuration.md#which-value-wins)
