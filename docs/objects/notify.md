# notify

A notify rule decides which [contacts](contact.md) get which
notifications. Each rule says who (`contacts`, `contact_groups`), about
what (`hosts`, `services`), and for which events (`states`, `on`).

```toml
[[notify]]
contacts = ["oncall"]
```

The simplest rule: `oncall` gets every notification about every host and
service.

How the rules work together:

- **Every rule that matches notifies.** There is no order and no "stop"
  rule.
- **Each contact gets one message**, even if several rules match it.
- A rule needs at least one contact or contact group.

Notify rules are written with double brackets, `[[notify]]`, because each
one is an entry in a list.

## Fields

| Field | Default | Description |
|---|---|---|
| `name` | | Name of the rule, shown in logs and the Config Tree, and used by `watcher config explain notify <name>`. Without a name, the rule shows as `rule at file:line`; it can still be edited in the web UI. |
| `contacts` | | Contacts that get the notifications. |
| `contact_groups` | | Contact groups that get the notifications. |
| `hosts` | all hosts | Which hosts the rule is about. See [Host selector](#host-selector). |
| `services` | all services | Which services the rule is about. See [Service selector](#service-selector). A rule with `services` gets no host notifications. |
| `states` | all problem states | Only problems in these states, and recoveries from them: `warning`, `critical`, `unknown`, `down`. |
| `on` | all events | Only these events: `problem`, `recovery`, `flapping`, `downtime`. |
| `enabled` | `true` | `false`: the rule is ignored. |

### states

| Value | Matches |
|---|---|
| `warning` | a service in WARNING |
| `critical` | a service in CRITICAL |
| `unknown` | a service in UNKNOWN |
| `down` | a host that is DOWN |

`states` filters problems and recoveries only. A recovery passes the
filter if the state it recovers from is in the list: a rule with
`states = ["critical"]` gets CRITICAL → OK, but not WARNING → OK.

Flapping and downtime notifications don't depend on `states`.

### on

| Value | Events |
|---|---|
| `problem` | PROBLEM: a host or service has a confirmed (HARD) problem, or the problem got worse, for example WARNING → CRITICAL |
| `recovery` | RECOVERY: back to OK or UP |
| `flapping` | FLAPPINGSTART, FLAPPINGSTOP: a service starts or stops flapping |
| `downtime` | DOWNTIMESTART, DOWNTIMEEND: a downtime starts or ends |

## Examples

### Everything to one contact

```toml
[[notify]]
contacts = ["oncall"]
```

### Only some hosts

```toml
[[notify]]
contacts = ["oncall"]
hosts = { groups = ["prod"] }
```

### Only some services

```toml
[[notify]]
contact_groups = ["dba"]
services = { checks = ["pgsql"] }
```

### Only critical problems

```toml
[[notify]]
name = "critical-only"
contacts = ["manager"]
states = ["critical", "down"]
```

Include `down` if the rule should also get hosts that are DOWN.

### No recoveries

```toml
[[notify]]
contacts = ["chatops"]
on = ["problem"]
```

### Problems and recoveries, no flapping or downtime messages

```toml
[[notify]]
contacts = ["oncall"]
on = ["problem", "recovery"]
```

### All hosts except some

```toml
[[notify]]
contacts = ["oncall"]
hosts = { groups = ["prod"], exclude = { name = "lab*" } }
```

### All services except some

```toml
[[notify]]
contacts = ["oncall"]
services = { exclude = { checks = ["disk"] } }
```

### A customer gets only their own hosts

```toml
[[notify]]
name = "acme"
contacts = ["acme-it"]
hosts = { groups = ["acme"] }
states = ["critical", "down"]
on = ["problem", "recovery"]
```

### Turned off

```toml
[[notify]]
contacts = ["oncall"]
enabled = false
```

## Host selector

`hosts` chooses which hosts a rule is about. Without `hosts`, the rule is
about all hosts.

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

By address:

```toml
hosts = { address = "10.0.1.*" }
```

By host group:

```toml
hosts = { groups = ["prod"] }
```

Host group and name, both must match:

```toml
hosts = { groups = ["prod"], name = "db*" }
```

By var:

```toml
hosts = { vars.os = ["debian", "ubuntu"] }
```

All of a group except some:

```toml
hosts = { groups = ["prod"], exclude = { name = "lab*" } }
```

## Service selector

`services` chooses which services a rule is about. Without `services`, the
rule is about all services and all hosts.

A rule with `services` is only about services: it never gets host
notifications (host DOWN and UP). To get those, add a second rule without
`services`.

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

By check:

```toml
services = { checks = ["disk"] }
```

By name:

```toml
services = { name = "disk_*" }
```

By service group:

```toml
services = { groups = ["webshop"] }
```

Only on some hosts:

```toml
services = { checks = ["http"], hosts = { groups = ["prod"] } }
```

If both `hosts` and `services` are set on the rule, both must match.

## Expressions

For conditions the fields can't express, for example an OR across two
different fields, use `expr`:

```toml
hosts = { expr = 'host.groups contains "db" or host.name ~ "^pg"' }
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

```toml
hosts = { expr = 'host.vars.os == "debian" && host.address ~ "^10\.0\."' }
```

```toml
hosts = { expr = 'host.name in ["db01", "web01"]' }
```

```toml
services = { expr = 'service.check == "disk" and not host.groups contains "test"' }
```

## When notifications are sent

A notification is sent when:

- a host or service changes to a confirmed (HARD) problem, including a
  change from WARNING to CRITICAL (PROBLEM)
- a confirmed problem goes back to OK or UP (RECOVERY)
- a service starts or stops flapping (FLAPPINGSTART, FLAPPINGSTOP)
- a downtime starts or ends (DOWNTIMESTART, DOWNTIMEEND)

No notification is sent for:

- problems that are not yet confirmed (SOFT states)
- the first OK result of a new service, and states restored after a
  restart
- the other services of a host that is DOWN (HARD): you get the host
  notification instead
- problems and recoveries while a service flaps
- a host check in WARNING (the host is still UP), and a host check changing
  between CRITICAL and UNKNOWN (the host stays DOWN)
- hosts and services that are muted, in a downtime or acknowledged, and
  everything while notifications are turned off globally (see
  [Operations](../guides/operations.md))

The host check notifies as the host: "web01 is DOWN".

*Not yet:* repeated notifications for ongoing problems, escalations.

## Checking a rule

What a rule matches:

```sh
watcher config explain notify dba-critical
```

Who gets notified about a service:

```sh
watcher config explain web01/http
```

```
└─ notifications
   └─ notify rule at conf.d/notify.toml:1: matched: host: group prod  notify.toml:1
      └─ contact oncall                                             contacts.toml:1
```

Unknown hosts, groups, checks or nodes in a selector give a warning, since
they are usually typos.

## See also

- [contact](contact.md), [contactgroup](contactgroup.md)
- [Notifications](../guides/notifications.md): SMTP, message format, webhooks
