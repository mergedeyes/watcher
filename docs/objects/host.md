# host

A host is a machine or device to monitor: a server, a switch, a website.
It gets services directly or through [host groups](hostgroup.md) and
[service groups](servicegroup.md).

```toml
[[host]]
name = "web01"
address = "10.0.0.5"
services = ["ping", "http"]
```

web01 gets the services `ping` and `http`, and their checks run against
10.0.0.5. Because the host has an address, its `ping` service also decides
whether the host is UP or DOWN, see [Host state](#host-state).

Hosts are written with double brackets, `[[host]]`, because each one is an
entry in a list. Each host needs a unique `name`.

## Fields

| Field | Default | Description |
|---|---|---|
| `name` | | Required. Unique name of the host. |
| `address` | the name | IP address or DNS name. Available in commands as `$host.address$`. |
| `groups` | | Host groups the host belongs to. See [hostgroup](hostgroup.md). |
| `services` | | Services the host gets. See [service](service.md). |
| `servicegroups` | | Service groups the host gets; it gets all their services. See [servicegroup](servicegroup.md). |
| `vars` | | Values for the `$name$` macros of the host's services, and the tables for `for_each`. Win over host group vars. [Built-in checks](check.md#built-in-checks) don't read them, except through macros like `$host.vars.port$`. |
| `host_check` | from the host groups; otherwise `ping` if `address` is set | The service that decides whether the host is UP or DOWN. `false`: no host check, the host is always UP. See [Host state](#host-state). |
| `graph` | the host check | The service whose graph is shown on the host page. |
| `ssh` | from the host groups | Login for [checks over SSH](../guides/ssh.md): `{ user, secret, port }`. See [ssh](#ssh). |
| `enabled` | `true` | `false`: the host is not monitored. |
| `node` | | Node label. *Not yet used* (nodes come in a later version). |
| `service.<name>` | | A host service: settings of one service on this host only. See [Host services](#host-services). |

## Examples

### A host with services

```toml
[[host]]
name = "web01"
address = "10.0.0.5"
services = ["http", "ssh"]
```

### A host in host groups

The host gets the services and settings of its groups.

```toml
[[host]]
name = "db01"
address = "10.0.2.21"
groups = ["linux", "databases"]
```

### A host with a service group

```toml
[[host]]
name = "web01"
servicegroups = ["linux-basics"]
```

### Different values on one host

The check `http` uses `$url$`. This host checks another URL:

```toml
[[host]]
name = "shop"
services = ["http"]
vars.url = "/health"
```

### One service per disk

The service `disk` has `for_each = "disks"`. The host gets `disk_root` and
`disk_data`. See [for_each](service.md#one-service-per-item-for_each).

```toml
[[host]]
name = "db01"
services = ["disk"]
vars.disks.root = { path = "/" }
vars.disks.data = { path = "/srv", warn = "10%" }
```

### A host without a host check

```toml
[[host]]
name = "printer"
address = "10.0.9.40"
services = ["snmp_toner"]
host_check = false
```

### Another graph on the host page

```toml
[[host]]
name = "app01"
graph = "load"
```

### A host that is not monitored

```toml
[[host]]
name = "old01"
enabled = false
```

Hosts and services that are disabled or removed from the configuration
keep their history in the database.

## Host services

`[host.service.<name>]` below a host holds settings for one service on this
host only. Write it after the host's other fields, because in TOML every
key below a `[host.service.x]` line belongs to that service.

What it does depends on the service:

- **The host already has the service** (from `services`, a host group or a
  service group): the settings change it on this host.
- **The host doesn't have the service, but `[service.<name>]` exists**: the
  host gets the service, with these settings.
- **Neither**: a new service that exists only on this host. Like a normal
  service, it uses the check with the same name unless `check` or
  `command` is set.

### Fields

A host service takes the same fields as a [service](service.md):

| Field | Default | Description |
|---|---|---|
| `check` | the service's check, or the check with the same name | The [check](check.md) to use. |
| `for_each` | the service's | Name of a host var. Creates one service per entry of that var. See [for_each](service.md#one-service-per-item-for_each). |
| `groups` | | More service groups for the service on this host. They add to the service's groups. |
| `enabled` | `true` | `false`: this host doesn't get the service. |
| `title` | the service's or check's `title` | Name shown in the web UI. Can contain [macros](check.md#macros). |
| `command` | the service's or check's | The command to run. Can contain [macros](check.md#macros). |
| `builtin` | the service's or check's | Use a check built into Watcher instead of a command: `ping`, `tcp`, `http`, `dns` or `tls`. See [Built-in checks](check.md#built-in-checks). |
| `shell` | `false` | `true` runs the command with `/bin/sh -c`, needed for pipes, `$(...)` or `&&`. |
| `run_on` | `local` | Where the command runs. `local`: on the Watcher machine. `host`: on the monitored host over SSH, see [Checks over SSH](../guides/ssh.md). |
| `copy` | `false` | With `run_on = "host"`: upload the script to the host before running it. |
| `passive` | `false` | `true`: Watcher runs nothing and waits for results sent over the API. See [Passive checks](check.md#passive-checks). |
| `format` | `auto` | How the output is read: `auto`, `nagios`, `key_value` or `json`. See [Output formats](check.md#output-formats). |
| `vars` | | Values for the `$name$` macros in the command, or the settings of a [built-in check](check.md#built-in-checks). Win over all other vars. |
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

Every value set here wins over the service's and the check's. All other
values still come from them.

### Changing a service on one host

```toml
[[host]]
name = "db01"
groups = ["linux"]
  [host.service.load]
  thresholds.load1 = { warn = "> 16" }
```

### Removing a service from one host

The host group `linux` gives every member `load`, but db01 shouldn't have
it:

```toml
[[host]]
name = "db01"
groups = ["linux"]
  [host.service.load]
  enabled = false
```

### Adding a defined service with other values

```toml
[[host]]
name = "web01"
  [host.service.disk_root]
  vars.warn = "30%"
```

### A service only this host has

```toml
[[host]]
name = "backup01"
  [host.service.backup_age]
  command = "/opt/scripts/backup_age.sh"
  interval = "1h"
```

### for_each services

With `for_each`, `[host.service.disk_data]` changes only the service for
the entry `data`, and `[host.service.disk]` changes all `disk_*` services
of the host.

```toml
[[host]]
name = "db01"
vars.disks.root = { path = "/" }
vars.disks.data = { path = "/srv" }
  [host.service.disk]
  interval = "10m"          # disk_root and disk_data
  [host.service.disk_data]
  enabled = false           # only disk_data
```

### Several host services

Each one starts with its own line:

```toml
[[host]]
name = "web01"
groups = ["linux"]
  [host.service.disk_root]
  vars.warn = "30%"
  [host.service.load]
  enabled = false
  [host.service.uptime]
  command = "uptime"
```

## Host state

A host is UP or DOWN. The state comes from one of its services, the host
check:

| State of the host check | Host state |
|---|---|
| OK, WARNING | UP |
| CRITICAL, UNKNOWN | DOWN |

Which service is the host check:

1. `host_check` on the host, if set.
2. Otherwise `host_check` from its host groups.
3. Otherwise, if the host has an `address`: its service `ping`. If it has
   no service `ping`, Watcher adds one using the
   [built-in ping check](check.md#built-in-checks).
4. Otherwise there is no host check, and the host is always UP.

`host_check = false` turns the host check off, also the automatic `ping`.

What the host state changes:

- While a host is HARD DOWN, its other services don't send notifications.
  You get one notification for the host instead of one per service.
- The host check notifies as the host: "web01 is DOWN", not
  "web01/ping is CRITICAL".
- Retries count only CRITICAL and UNKNOWN results. A WARNING result means
  the host is UP and ends the retries.
- A WARNING of the host check doesn't notify, because the host is still UP.

The host check must be a service the host has; otherwise it's an error.

## ssh

The login for [checks over SSH](../guides/ssh.md), used by checks with
`run_on = "host"`. Watcher connects to the host's `address`.

| Field | Default | Description |
|---|---|---|
| `user` | | Required. User to log in as. |
| `secret` | | Required. Name of a key or password stored in Watcher. See [Secrets](../guides/ssh.md#secrets). |
| `port` | `22` | SSH port. |

The login is usually set once on a host group. A host can set some fields
itself and take the rest from its host groups:

```toml
[hostgroup.linux]
members = ["web*", "db*"]
ssh = { user = "monitor", secret = "monitor" }

[[host]]
name = "db07"
groups = ["linux"]
ssh = { port = 2222 }          # user and secret from linux
```

## See also

- [hostgroup](hostgroup.md): settings for many hosts at once
- [How a host gets its services](../guides/configuration.md#how-a-host-gets-its-services)
- [Which value wins](../guides/configuration.md#which-value-wins)
- [Checks over SSH](../guides/ssh.md)
