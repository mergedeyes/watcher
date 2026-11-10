# dependency

A dependency holds notifications back while another host or service has
a problem. When a router is down, every host behind it fails too — the
dependency keeps those alerts quiet, so you get one notification about
the router instead of fifty about the hosts behind it.

```toml
[[dependency]]
name = "behind-gw"
on = "gw"
hosts = { groups = ["branch-office"] }
```

While the host `gw` is DOWN, hosts in the `branch-office` group and
their services send no notifications. When `gw` is UP again,
notifications flow again.

The host or service after `on` is called the *master*; the hosts and
services that wait on it are the *dependents*. Checks keep running for
everything: only notifications are held back, and the web UI keeps
showing the real states.

Dependencies are written with double brackets, `[[dependency]]`, because
each one is an entry in a list.

## Fields

| Field | Default | Description |
|---|---|---|
| `name` | | Name of the dependency, shown in logs and the Config Tree, and used by `watcher config explain dependency <name>`. Without a name, it shows as `dependency at file:line`; it can still be edited in the web UI. |
| `on` | required | The master: a host name (`"gw"`) or a `host/service` ID (`"db01/postgres"`). It must exist. |
| `hosts` | all hosts | Which hosts wait on the master. See [Host selector](#host-selector). |
| `services` | all services | Which services wait on the master. See [Service selector](#service-selector). A dependency with `services` holds back no host notifications. |
| `states` | see below | Master states that hold notifications back. For a master host: `down`. For a master service: `warning`, `critical`, `unknown`; the default is `["critical", "unknown"]`. |
| `enabled` | `true` | `false`: the dependency is ignored. |

### states

For a master host, the only state is `down`, and it is the default: the
dependents wait while the host is DOWN.

For a master service, the defaults are `critical` and `unknown`. A
master in WARNING holds nothing back — it still works, just not well.
Add `warning` to the list if a degraded master should silence its
dependents too.

## How it works

- Notifications for a dependent are held back while the master is in one
  of the listed states and that state is confirmed (HARD). A master in a
  SOFT state holds nothing back.
- A master host is DOWN or UP through its [host
  check](host.md#host-state). A master host without a host check is
  always UP, so the dependency never applies; loading warns about it.
- The master's own notifications are never held back: the alert that
  tells you the master is down always goes out.
- Checks keep running. The dependents' states stay visible in the web
  UI; only their notifications wait.
- [Reminders](notify.md#repeat) and [escalations](notify.md#after) for a
  held-back problem wait too. Their time keeps running: a problem that
  outlives the master's outage is sent once the master has recovered.
- A dependent that recovers while it is held back sends nothing — there
  is no recovery to report once the master is back and the dependent is
  simply OK. Contacts see the state in the web UI.
- The log says why nothing was sent: `no notification for web01/http:
  depends on gw (DOWN)`.

A host that is DOWN already silences its own services, without any
configuration: see [When notifications are
sent](notify.md#when-notifications-are-sent). Dependencies are for
problems that reach *across* hosts: hosts behind a router, services that
need a database on another machine.

## Examples

### Hosts behind a router

```toml
[[dependency]]
name = "behind-gw"
on = "gw"
hosts = { groups = ["branch-office"] }
```

While `gw` is DOWN: no notifications about the hosts in
`branch-office`, neither host alerts nor their services.

### Services that need a database

```toml
[[dependency]]
name = "needs-db"
on = "db01/postgres"
services = { checks = ["http"], hosts = { groups = ["webshop"] } }
```

While `db01/postgres` is CRITICAL or UNKNOWN (confirmed), the `http`
services of the `webshop` hosts stay quiet.

### Everything on one host waits for a service there

```toml
[[dependency]]
on = "nas/raid"
services = { hosts = ["nas"] }
```

### A degraded master silences too

```toml
[[dependency]]
on = "db01/postgres"
services = { checks = ["http"] }
states = ["warning", "critical", "unknown"]
```

### Turned off

```toml
[[dependency]]
name = "behind-gw"
on = "gw"
hosts = { groups = ["branch-office"] }
enabled = false
```

## Host selector

`hosts` chooses which hosts wait on the master. Without `hosts`, all
hosts do (except the master's own alert).

| Field | Matches hosts |
|---|---|
| `name` | whose name matches. One pattern or a list. |
| `address` | whose address matches. One pattern or a list. |
| `groups` | in any of these host groups |
| `vars.<name>` | whose var has this value, or any value of a list |
| `node` | with this node label, or any of a list |
| `exclude` | Another host selector. Hosts it matches are removed. |
| `expr` | for which the [expression](#expressions) is true |

How the fields combine:

- All fields you set must match (AND).
- A list matches if any of its values matches (OR).
- In names and addresses, `*` matches any text and `?` one character.
- `{}` matches all hosts.

By name:

```toml
hosts = { name = "web*" }
```

A list of names, short form:

```toml
hosts = ["web01", "db*"]
```

By host group:

```toml
hosts = { groups = ["branch-office"] }
```

All of a group except some:

```toml
hosts = { groups = ["branch-office"], exclude = { name = "lab*" } }
```

## Service selector

`services` chooses which services wait on the master. Without
`services`, all services and all hosts do.

A dependency with `services` only holds back service notifications: host
notifications (host DOWN and UP) still go out. To hold those back too,
add a second dependency without `services`.

| Field | Matches services |
|---|---|
| `name` | whose name matches. One pattern or a list. |
| `checks` | that use any of these checks |
| `groups` | in any of these service groups |
| `hosts` | on hosts that match this [host selector](#host-selector) |
| `exclude` | Another service selector. Services it matches are removed. |
| `expr` | for which the [expression](#expressions) is true |

The same rules apply as for hosts: all fields must match, a list matches
any of its values, `*` and `?` work in names.

A list of services, short form. Each entry is a service name or a
`host/service` ID, and patterns work:

```toml
services = ["web01/http", "db*/pgsql", "backup"]
```

By check, only on some hosts:

```toml
services = { checks = ["http"], hosts = { groups = ["webshop"] } }
```

If both `hosts` and `services` are set on the dependency, both must
match.

## Expressions

For conditions the fields can't express, for example an OR across two
different fields, use `expr`:

```toml
hosts = { expr = 'host.groups contains "branch" or host.name ~ "^shop"' }
```

| Value | Type |
|---|---|
| `host.name`, `host.address`, `host.node` | text |
| `host.groups` | list |
| `host.vars.x`, `host.vars.x.y` | var, nested var |
| `service.name`, `service.check`, `service.groups` | text, text, list. Only in service selectors. |

| Operator | Meaning |
|---|---|
| `==`, `!=` | equal, not equal |
| `~`, `!~` | matches a regular expression, doesn't match |
| `contains` | list contains a value |
| `in` | value is in a list `[...]` |
| `and` or `&&` | both |
| `or` or `\|\|` | either |
| `not` or `!` | negation |
| `( )` | grouping |

Text goes in `"..."` or `'...'`. A var that isn't set never matches, so
`!=` on an unset var is true.

## Checking a dependency

What a dependency holds back, and what it waits on:

```sh
watcher config explain dependency behind-gw
```

```
behind-gw
├─ on
│  └─ host gw: holds back while DOWN
└─ holds back
   ├─ host web01: host: group branch-office
   └─ service web01/http: host: group branch-office
```

A service's Config Tree shows the dependencies that apply to it:

```sh
watcher config explain web01/http
```

```
└─ dependencies
   └─ dependency behind-gw: matched: host: group branch-office; held back while gw is DOWN
```

Unknown hosts, groups, checks or nodes in a selector give a warning,
since they are usually typos.

## See also

- [notify](notify.md): who gets which notifications
- [host](host.md): the host check, which gives a host its UP/DOWN state
- [Notifications](../guides/notifications.md)
- [Alerting recipes](../guides/alerting-recipes.md): complete setups for common cases
