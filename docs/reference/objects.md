# Object files

Every `*.toml` below `conf_dir` (default `conf.d/`), hidden entries
skipped. Any file may hold any kind. Saved files reload within a second; a
config with errors is never applied.

Layout by convention:

```
conf.d/
  checks.toml  roles.toml  hosts.toml
  groups.toml  contacts.toml  notify.toml  sinks.toml
  ui/          objects created in the web UI
```

| Kind | TOML |
|---|---|
| [check](#check) | `[check.<name>]` |
| [role](#role) | `[role.<name>]` |
| [host](#host) | `[[host]]` |
| [service](#service) | `role.<r>.service.<name>`, `host.service.<name>` |
| [hostgroup](#hostgroup) | `[hostgroup.<name>]` |
| [servicegroup](#servicegroup) | `[servicegroup.<name>]` |
| [contact](#contact) | `[contact.<name>]` |
| [contactgroup](#contactgroup) | `[contactgroup.<name>]` |
| [notify](#notify) | `[[notify]]` |
| [sink](#sink) | `[[sink]]` |

**Names:** not empty, no `/`, no control characters, no leading or
trailing spaces. Var names and `for_each` keys: `[a-z_][a-z0-9_]*`.

**Durations:** `30s`, `5m`, `1h`, `1day`, `1h 30m`.

## check

```toml
[check.http]
title = "HTTP"
command = "check_http -H $host.address$ -u $url$"
vars.url = "/"
thresholds.time = { warn = "> 500ms", crit = "> 2s" }
interval = "1m"
```

| Field | Default | |
|---|---|---|
| `title` | | display name |
| `command` | | required unless `passive`. [Macros](../guides/checks.md#macros) |
| `shell` | `false` | run with `/bin/sh -c` |
| `passive` | `false` | results via API, no command |
| `format` | `auto` | `auto`, `nagios`, `key_value`, `json` |
| `vars` | | `$name$` values; scalars or tables |
| `thresholds.<metric>` | | `{ warn = "...", crit = "..." }` ([syntax](../guides/checks.md#thresholds)) |
| `interval` | `1m` | |
| `retry_interval` | min(interval, 1m) | while SOFT |
| `max_attempts` | 3, passive 1 | |
| `timeout` | `60s` | |
| `freshness` | | passive: UNKNOWN without a result for this long |
| `flap_detection` | `true` | |
| `flap_low` | 5 | percent, stop flapping below |
| `flap_high` | 20 | percent, start flapping at |

A check used by nothing: warning.

## role

```toml
[role.web]
title = "Web servers"
include = ["linux"]
hosts = { tags = ["web"] }
host_check = "ping"
graph = "http"
vars.url = "/"
groups = ["web-servers"]
interval = "1m"
service.http = {}
```

| Field | |
|---|---|
| `title` | display name |
| `include` | roles merged into this one; this one wins. Cycles: error |
| `hosts` | [host selector](selectors.md#host-selector): hosts that get the role. No `roles`/`groups` |
| `host_check` | service giving its hosts UP/DOWN |
| `graph` | service graphed on the host page |
| `node` | node label for its hosts |
| `vars` | host var defaults |
| `groups` | host groups its hosts join |
| `interval`, `retry_interval`, `max_attempts`, `timeout` | defaults for its services |
| `service.<name>` | [service](#service) |

A role used by no host: warning.

## host

```toml
[[host]]
name = "db01"
address = "10.0.2.21"
roles = ["linux", "db"]
tags = ["prod"]
groups = ["dc1"]
vars.disks.data = { path = "/srv" }
  [host.service.backup]
  passive = true
  freshness = "26h"
```

| Field | Default | |
|---|---|---|
| `name` | | required, unique |
| `address` | the name | `$host.address$` |
| `roles` | | roles listed on the host |
| `tags` | | free labels for selectors; add up with roles' |
| `groups` | | host groups |
| `vars` | | `$name$` values, `for_each` tables |
| `node` | | node label (*not yet*: nodes) |
| `host_check` | from roles | service giving UP/DOWN |
| `graph` | the host check | service graphed on the host page |
| `enabled` | `true` | `false`: not monitored |
| `service.<name>` | | own service or override, [service](#service) |

## service

In `[role.<r>]` as `service.<name> = { ... }`, on a host as
`[host.service.<name>]`.

```toml
[role.linux]
service.disk = { check = "disk", for_each = "disks", groups = ["storage"] }

[[host]]
name = "web01"
  [host.service.disk_root]
  vars.warn = "30%"
```

| Field | Default | |
|---|---|---|
| `check` | the service name | check to use; none with own `command` or `passive` |
| every [check](#check) field | | overrides the check's |
| `for_each` | | host var with a table of tables: one service `<name>_<key>` per entry |
| `groups` | | service groups |
| `enabled` | `true` | `false` on a host: removes the role's service there |

On a host, `service.<name>` overrides the role service of that name (with
`for_each`: `<name>_<key>` one, `<name>` all), or adds a new one.

Precedence: host > role > included role > check > defaults, key by key.
Tables merge per key; lists are replaced, except `tags` and `groups`.
Two roles on a host setting the same value differently: error.

## hostgroup

```toml
[hostgroup.dc1]
title = "Datacenter 1"
members = { address = "10.0.*" }
```

| Field | |
|---|---|
| `title` | display name |
| `members` | [host selector](selectors.md#host-selector), no `groups` |

Hosts also join with host `groups` and role `groups`.

## servicegroup

```toml
[servicegroup.webshop]
title = "Webshop"
members = ["web01/http", "db01/pgsql"]
```

| Field | |
|---|---|
| `title` | display name |
| `members` | [service selector](selectors.md#service-selector) |

Services also join with service `groups`.

## contact

```toml
[contact.alice]
email = ["alice@example.com", "alice@pager.example.com"]
webhook = "https://hooks.example.com/alice"
groups = ["dba"]
```

| Field | |
|---|---|
| `email` | one address or a list |
| `webhook` | URL, POST JSON ([format](../guides/notifications.md#webhook)) |
| `groups` | contact groups |

## contactgroup

```toml
[contactgroup.dba]
title = "DBAs"
members = ["alice", "bob"]
```

| Field | |
|---|---|
| `title` | display name |
| `members` | contact names |

## notify

```toml
[[notify]]
name = "dba-critical"
contact_groups = ["dba"]
services = { groups = ["webshop"] }
states = ["critical"]
on = ["problem", "recovery"]
```

| Field | Default | |
|---|---|---|
| `name` | | needed to edit it in the UI |
| `contacts` | | |
| `contact_groups` | | |
| `hosts` | all | [host selector](selectors.md#host-selector) |
| `services` | all | [service selector](selectors.md#service-selector) |
| `states` | all | `warning`, `critical`, `unknown`, `down` |
| `on` | all | `problem`, `recovery`, `flapping` |
| `enabled` | `true` | |

Every matching rule notifies; each contact gets one message.
[Notifications](../guides/notifications.md).

## sink

```toml
[[sink]]
name = "central"
type = "postgres"
url = "postgres://watcher@central.example.com/watcher"
password_file = "central.pass"
flush = { mode = "interval", interval = "1m" }
buffer = { max_size = "1GB", on_full = "drop_oldest" }
```

| Field | Default | |
|---|---|---|
| `name` | | required, unique |
| `type` | | `postgres` (*not yet*: `hub`) |
| `url` | | `postgres://` or `postgresql://` |
| `password_file` | | relative to this file |
| `flush.mode` | `interval` | `interval`, `realtime` (*not yet*: `adaptive`, `schedule`) |
| `flush.interval` | `1m` | |
| `buffer.max_size` | `1GB` | `512MB`, `2GB` (×1024) or bytes |
| `buffer.on_full` | `drop_oldest` | (*not yet*: `downsample`, `block`) |

[Data](../guides/data.md#sinks).
