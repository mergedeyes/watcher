# notify

A notify rule decides which [contacts](contact.md) get which
notifications. Each rule says who (`contacts`, `contact_groups`), about
what (`hosts`, `services`), for which events (`states`, `on`), whether
open problems are sent again (`repeat`), whether the rule only steps in
when a problem lasts (`after`), and when it applies (`period`).

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

For complete setups (quiet start, reminders, escalation ladders, one
team per service, maintenance windows), see
[Alerting recipes](../guides/alerting-recipes.md).

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
| `repeat` | off | Send an open problem again this often, for example `"1h"`. At least `1m`; `"0"` turns it off. See [repeat](#repeat). |
| `after` | at once | Makes the rule an escalation: its contacts get a problem only once it has been open this long and nobody acknowledged it, for example `"30m"`. `"0"` turns it off. See [after](#after). |
| `period` | always | Name of a [time period](timeperiod.md). The rule only notifies inside it; open problems are sent when it begins. See [period](#period). |
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

### repeat

With `repeat`, the rule's contacts get a problem again and again while it
is still open, until it is OK (or UP) again:

```toml
[[notify]]
contacts = ["oncall"]
repeat = "1h"
```

`oncall` gets the PROBLEM when it happens, then "still CRITICAL" every
hour. The subject says `is still`:

```
[Watcher] PROBLEM: web01/http is still CRITICAL
```

How the time is counted:

- The hour counts from the last notification about the problem that the
  contact got. A change, for example WARNING → CRITICAL, is a new PROBLEM
  and starts the hour again.
- A contact that never got the problem, for example because it started
  during a downtime, gets it once `repeat` has passed since the problem
  started. So problems hidden by a downtime are sent when the downtime is
  over, if they have been open long enough.
- If several rules with `repeat` give a contact the same problem, the
  shortest time applies.
- With a database, a restart changes nothing: Watcher reads from the
  [notification log](../guides/notifications.md#notification-log) who got
  which problem when, and goes on from there. A repeat that fell due while
  Watcher was stopped goes out right after the start, once.
- Without a database, the time starts again for problems that are already
  open. A restart never sends a wave of repeated notifications.

No repeats are sent while the problem is acknowledged, in a downtime,
muted, or flapping, while its host is DOWN, while a
[dependency](dependency.md) holds it back, or while notifications are
turned off. The time keeps running: when the acknowledgement is removed
and the hour is over, the next repeat goes out within a minute.

`repeat` works together with `states` and `on`: a rule with
`states = ["critical"]` repeats only CRITICAL problems, and `on` must
include `problem`.

### after

With `after`, a rule is an *escalation*. Its contacts don't get a problem
when it happens. They get it only if it is still open after that time and
nobody took care of it:

```toml
[[notify]]
contacts = ["oncall"]

[[notify]]
name = "escalate-to-lead"
contacts = ["teamlead"]
after = "30m"
```

`oncall` gets the PROBLEM at once. If the problem is still open 30
minutes later, and not acknowledged, `teamlead` gets it too:

```
[Watcher] PROBLEM: web01/http is still CRITICAL
```

How it works:

- The time counts from when the problem reached its current state. A
  change, for example WARNING → CRITICAL, starts it again, and the
  escalation contacts get the new state once it has lasted `after`.
- A problem that is acknowledged, in a downtime, muted or flapping, whose
  host is DOWN, held back by a [dependency](dependency.md), or while
  notifications are turned off, is not escalated.
  The time keeps running: if the acknowledgement is removed after the 30
  minutes, the escalation goes out within a minute.
- With `repeat` on the same rule, the escalation contacts get the problem
  again every `repeat` after that.
- The escalation contacts get the RECOVERY only if they got the problem.
  Use `on = ["problem"]` if they shouldn't get recoveries at all.
- Contacts who already got the problem from another rule are not sent it
  again by the escalation.
- With a database, a restart changes nothing, as with `repeat`: an
  escalation that fell due while Watcher was stopped goes out right after
  the start, and contacts who got the problem before the restart still get
  the RECOVERY.
- Without a database, the time starts again after a restart for problems
  that are already open, and escalations sent before the restart get no
  RECOVERY.

Several rules with different `after` times make escalation levels, for
example the team lead after 30 minutes and the manager after 2 hours.

Escalation rules only take problems and recoveries. `on` must include
`problem`, and can't include `flapping` or `downtime`.

### period

With `period`, a rule only notifies inside a [time period](timeperiod.md),
for example business hours:

```toml
[timeperiod.workhours]
schedule = [{ days = ["mon", "tue", "wed", "thu", "fri"], from = "08:00", to = "18:00" }]

[[notify]]
contact_groups = ["team"]
period = "workhours"
```

How it works:

- Outside the time period, the rule's contacts get nothing.
- When it begins, they get the problems that are still open and that they
  didn't get yet: a server that broke at 03:00 and is still broken is
  sent at 08:00. Problems that recovered or were acknowledged in the
  meantime are not sent.
- They get the RECOVERY only if they got the problem, at any time.
- `repeat` and `after` of the rule only send inside the time period.
- Flapping and downtime messages are sent only inside the time period.

Two rules with opposite time periods route notifications by time of day,
see [Alerting recipes](../guides/alerting-recipes.md#on-call-at-night-the-team-during-business-hours).

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

### Remind until someone acknowledges

```toml
[[notify]]
contacts = ["oncall"]
repeat = "30m"
```

### Remind the manager only about critical problems

```toml
[[notify]]
contacts = ["oncall"]

[[notify]]
name = "manager-reminder"
contacts = ["manager"]
states = ["critical", "down"]
on = ["problem"]
repeat = "4h"
```

`oncall` gets every notification once. `manager` gets critical problems
and hosts that are DOWN, and again every 4 hours while they last.

### Escalate when nobody reacts

```toml
[[notify]]
contacts = ["oncall"]
repeat = "30m"

[[notify]]
name = "teamlead"
contacts = ["teamlead"]
after = "1h"

[[notify]]
name = "manager"
contacts = ["manager"]
states = ["critical", "down"]
after = "4h"
repeat = "4h"
```

`oncall` gets problems at once and every 30 minutes. If a problem is still
open and not acknowledged after an hour, `teamlead` gets it. Critical
problems still open after 4 hours also reach `manager`, then every 4
hours. Acknowledging the problem stops all of it.

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
| `node` | on this [node](../guides/nodes.md), or any of a list. `local`: hosts without a node, checked by this Watcher itself. |
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
| `host.name`, `host.address`, `host.node` | text. `host.node` is `local` for hosts without a node. |
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
- a problem is still open after `repeat`, if the rule has one (PROBLEM,
  "is still"; see [repeat](#repeat))
- a problem is still open after `after`, for escalation rules (PROBLEM,
  "is still"; see [after](#after))
- a time period begins while a problem is open that the rule's contacts
  didn't get yet (PROBLEM, "is still"; see [period](#period))

No notification is sent for:

- problems that are not yet confirmed (SOFT states)
- the first OK result of a new service, and states restored after a
  restart
- the other services of a host that is DOWN (HARD): you get the host
  notification instead
- hosts and services whose master in a [dependency](dependency.md) has a
  confirmed problem: you get the notification about the master instead
- problems and recoveries while a service flaps
- a host check in WARNING (the host is still UP), and a host check changing
  between CRITICAL and UNKNOWN (the host stays DOWN)
- hosts and services that are muted, in a downtime or acknowledged, and
  everything while notifications are turned off globally (see
  [Operations](../guides/operations.md))

The host check notifies as the host: "web01 is DOWN".

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

A rule with `after`, `repeat` or `period` adds it to the line, for example
`matched: host: group prod; escalates after 30m; repeats every 1h; only in workhours`.

Unknown hosts, groups, checks or nodes in a selector give a warning, since
they are usually typos.

## See also

- [contact](contact.md), [contactgroup](contactgroup.md)
- [timeperiod](timeperiod.md): when rules with `period` apply
- [dependency](dependency.md): quiet dependents while their master is down
- [Notifications](../guides/notifications.md): SMTP, message format, webhooks
- [Alerting recipes](../guides/alerting-recipes.md): complete setups for common cases
