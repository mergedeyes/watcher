# Object files

Every `*.toml` below `conf_dir` (default `conf.d/`), hidden entries
skipped. Any file may hold any kind. Saved files reload within a second; a
config with errors is never applied.

Layout by convention:

```
conf.d/
  checks.toml  services.toml  hosts.toml
  groups.toml  contacts.toml  notify.toml  sinks.toml
  ui/          objects created in the web UI
```

| Kind | TOML |
|---|---|
| [check](#check) | `[check.<name>]` |
| [service](#service) | `[service.<name>]` |
| [host](#host) | `[[host]]` |
| [host service](#host-service) | `[host.service.<name>]` |
| [hostgroup](#hostgroup) | `[hostgroup.<name>]` |
| [servicegroup](#servicegroup) | `[servicegroup.<name>]` |
| [contact](#contact) | `[contact.<name>]` |
| [contactgroup](#contactgroup) | `[contactgroup.<name>]` |
| [downtimegroup](#downtimegroup) | `[downtimegroup.<name>]` |
| [notify](#notify) | `[[notify]]` |
| [sink](#sink) | `[[sink]]` |

How a host gets a service:

```
check ─▶ service ─▶ host
            │         ▲
            ▼         │
     servicegroup ─▶ hostgroup
```

`services = [...]` and `servicegroups = [...]` on a host or host group.

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
| `title` | | display name. [Macros](../guides/checks.md#macros) |
| `command` | | required unless `passive` or `builtin`. [Macros](../guides/checks.md#macros) |
| `builtin` | | `ping`: [built-in check](../guides/checks.md#built-in-checks) instead of a command |
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
| `graphs` | `true` | `false`: no graphs |

A check used by nothing: warning.

## service

```toml
[service.disk_root]
check = "disk"
vars.path = "/"
groups = ["linux-basics"]

[service.disk]
for_each = "disks"
```

| Field | Default | |
|---|---|---|
| `check` | check of the same name | none with own `command` or `passive` |
| every [check](#check) field | | overrides the check's |
| `for_each` | | host var with a table of tables, dotted if nested (`site.tld`): one service `<name>_<key>` per entry; a `title` without macros gets ` (<key>)` |
| `groups` | | service groups |
| `enabled` | `true` | |

A service on no host: warning.

## host

```toml
[[host]]
name = "db01"
address = "10.0.2.21"
groups = ["linux", "dc1"]
services = ["backup"]
servicegroups = ["postgres"]
vars.disks.data = { path = "/srv" }
```

| Field | Default | |
|---|---|---|
| `name` | | required, unique |
| `address` | the name | `$host.address$` |
| `groups` | | host groups |
| `services` | | services |
| `servicegroups` | | their services |
| `vars` | | `$name$` values, `for_each` tables; over host group vars |
| `node` | | node label (*not yet*: nodes) |
| `host_check` | from host groups; with `address`: `ping` | service giving UP/DOWN; `false`: none |
| `graph` | the host check | service graphed on the host page |
| `enabled` | `true` | `false`: not monitored |
| `service.<name>` | | [host service](#host-service) |

## host service

`[host.service.<name>]`: settings of one service on one host.

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

- A service the host has: overrides it (with `for_each`: `<name>_<key>`
  one, `<name>` all).
- A service it doesn't have: assigns it, with the overrides.
- Neither: a one-off service on this host; `check` defaults to the check
  of the same name.

Fields: as [service](#service); `enabled = false` removes it from the host.

**Precedence**, key by key: host service > host vars > host group vars >
service > check > defaults. Tables merge per key; lists are replaced,
except `groups`. Two host groups of a host setting a value differently:
error.

## hostgroup

```toml
[hostgroup.linux]
title = "Linux Servers"
members = ["web*", "db01"]
servicegroups = ["linux-basics"]
services = ["ssh"]
host_check = "ping"
vars.warn = "20%"
```

| Field | |
|---|---|
| `title` | display name |
| `members` | host names; `*` and `?` match |
| `services` | services for every member |
| `servicegroups` | their services, for every member |
| `host_check` | service giving its hosts UP/DOWN; `false`: none |
| `graph` | service graphed on the host page |
| `vars` | host var defaults |

Hosts also join with host `groups`.

## servicegroup

```toml
[servicegroup.linux-basics]
title = "Linux Basics"
services = ["ping", "load", "disk_root"]
```

| Field | |
|---|---|
| `title` | display name |
| `services` | its services |

Services also join with service `groups`. A host or host group listing the
group gets all of them; the group shows all of them on every host.

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

## downtimegroup

```toml
[downtimegroup.patchday]
title = "Patch Day"
members = ["web*"]
hostgroups = ["linux"]
schedule = [{ days = ["sun"], from = "02:00", to = "04:00" }]
```

| Field | |
|---|---|
| `title` | display name |
| `members` | host names; `*` and `?` match |
| `hostgroups` | their hosts |
| `schedule` | windows in local time: `{ days, from, to }`; `days` `mon`…`sun` (none: daily); `to` not after `from`: next day; dates in `from`/`to` (`2026-12-24 00:00`): once |

No notifications for its hosts and their services during a window.
[Operations](../guides/operations.md#downtime-groups).

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
| `on` | all | `problem`, `recovery`, `flapping`, `downtime` (start and end) |
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
