# Alerting recipes

Ready-made notification setups for situations most teams run into. Each
recipe has the goal, the configuration, what happens over time, and what
to know about acknowledgements, problems that fix themselves, and
restarts.

The fields used here are explained in [notify](../objects/notify.md). The
contacts in the examples (`team`, `lead`, ...) need to exist as
[contacts](../objects/contact.md) or [contact groups](../objects/contactgroup.md).

Three things are true for every recipe:

- **Only confirmed problems notify.** A problem counts once it is HARD,
  after `max_attempts` failed checks in a row. The times below start
  there, not at the first failed check.
- **Acknowledging stops everything** for that problem: reminders and
  escalations. The RECOVERY is still sent, and it ends the
  acknowledgement.
- **A recovery goes to whoever got the problem.** Rules with `after`
  only send the RECOVERY to contacts who were told about the problem.

## Quiet start, team reminders, then the team lead

*Small problems often fix themselves. Give them 15 minutes. If the problem
is still there, tell the team, and remind them every 30 minutes. If nobody
has acknowledged it after 1.5 hours, tell the team lead.*

```toml
[[notify]]
name = "team"
contact_groups = ["team"]
after = "15m"
repeat = "30m"

[[notify]]
name = "teamlead"
contacts = ["lead"]
after = "1h30m"
```

For a problem that is confirmed at 10:00:

| Time | Who gets it |
|---|---|
| 10:00 | nobody yet |
| 10:15 | team |
| 10:45 | team ("is still CRITICAL") |
| 11:15 | team |
| 11:30 | lead |
| 11:45 | team, then every 30 minutes |

- **Acknowledged at 10:50:** nothing more is sent. The lead never hears
  about it. At recovery, the team gets the RECOVERY and the lead doesn't.
- **Acknowledged at 12:00, OK at 12:20:** team and lead both get the
  RECOVERY, since both got the problem.
- **OK again at 10:10:** nobody gets anything, neither the problem nor the
  recovery.
- **WARNING turns into CRITICAL:** that is a new problem. The team gets it
  15 minutes later, the lead 1.5 hours later.
- Add `repeat = "1h"` to the `teamlead` rule if the lead should be
  reminded too.

## Remind until someone acknowledges

*Tell the on-call person at once, and again every 30 minutes until they
acknowledge it.*

```toml
[[notify]]
contacts = ["oncall"]
repeat = "30m"
```

The reminder says how long the problem has been open:
`PROBLEM: web01/http is still CRITICAL (for 1h 30m)`. Acknowledge the
problem to stop the reminders.

## Escalation ladder

*On-call first. The team lead if nobody reacts within 30 minutes. The
manager only for critical problems that are still open after 2 hours.*

```toml
[[notify]]
contacts = ["oncall"]
repeat = "30m"

[[notify]]
contacts = ["lead"]
after = "30m"
repeat = "1h"

[[notify]]
contacts = ["manager"]
states = ["critical", "down"]
after = "2h"
```

Each level is one rule with its own `after`. `states` keeps WARNING
problems away from the manager; include `down` so hosts that are DOWN
count as well.

## On-call at night, the team during business hours

*During business hours, the team gets problems. At night and on weekends,
only the on-call person. Whatever is still broken in the morning goes to
the team when they start.*

```toml
[timeperiod.workhours]
title = "Business hours"
schedule = [{ days = ["mon", "tue", "wed", "thu", "fri"], from = "08:00", to = "18:00" }]

[timeperiod.offhours]
title = "Nights and weekends"
schedule = [
  { days = ["mon", "tue", "wed", "thu", "fri"], from = "00:00", to = "08:00" },
  { days = ["mon", "tue", "wed", "thu", "fri"], from = "18:00", to = "00:00" },
  { days = ["sat", "sun"], from = "00:00", to = "00:00" },
]

[[notify]]
contact_groups = ["team"]
period = "workhours"

[[notify]]
contacts = ["oncall"]
period = "offhours"
repeat = "30m"
```

For a problem that starts on Tuesday at 03:00:

| Time | Who gets it |
|---|---|
| 03:00 | oncall |
| 03:30, 04:00, ... | oncall again, every 30 minutes |
| 08:00 | team; oncall's reminders pause until 18:00 |

- **Acknowledged by on-call at 04:10:** nothing more is sent. The team
  doesn't get it at 08:00 either; someone is already on it.
- **Fixed at 06:00:** on-call gets the RECOVERY. The team never hears of
  it.
- **Problem at 17:55, fixed at 19:00:** the team got the problem, so they
  get the RECOVERY too, even after hours. On-call gets the problem at
  18:00, when their time period begins, and the RECOVERY at 19:00.
- The time zone is `timezone` in
  [watcher.toml](../reference/watcher-toml.md), or the system's.

## Managers: critical problems only, no recoveries

*Managers want to know about real outages, not about every WARNING, and
they don't need the all-clear.*

```toml
[[notify]]
contacts = ["manager"]
states = ["critical", "down"]
on = ["problem"]
```

Without `on`, the rule would also send recoveries, flapping and downtime
messages.

## Chat for everything, email only for critical problems

*The team chat gets every notification. Email only for CRITICAL and DOWN,
so the inbox stays quiet.*

A contact gets the same message on all its channels, so use two contacts:

```toml
[contact.ops-chat]
slack = "https://hooks.slack.com/services/T0000/B0000/XXXXXXXX"

[contact.ops-mail]
email = "ops@example.com"

[[notify]]
contacts = ["ops-chat"]

[[notify]]
contacts = ["ops-mail"]
states = ["critical", "down"]
on = ["problem", "recovery"]
```

`teams`, `telegram` or any other [channel](notifications.md#chat-push-and-paging-services)
works the same way. For paging, a contact with `pagerduty` in place of
`email` opens an incident for each critical problem and resolves it on
recovery.

## Each team gets its own services

*The DBAs get database problems, the web team gets web problems, and the
NOC sees everything.*

```toml
[[notify]]
contact_groups = ["dba"]
services = { checks = ["pgsql", "mysql"] }

[[notify]]
contact_groups = ["web"]
services = { groups = ["websites"] }

[[notify]]
contacts = ["noc"]
```

A rule with `services` never gets host notifications (host DOWN and
UP). If the DBAs should also hear when a database server goes down, add a
rule with `hosts = { groups = ["db-servers"] }` and without `services`.

## Customers see only their own hosts

*Each customer gets notifications about their own hosts only.*

```toml
[[notify]]
contacts = ["acme-it"]
hosts = { groups = ["acme"] }
states = ["critical", "down"]

[[notify]]
contacts = ["globex-it"]
hosts = { name = "globex-*" }
```

See [Customers (MSP)](../examples.md#customers-msp) for a complete setup.

## Maintenance windows without noise

*No alerts during the Sunday night patch window, but problems that are
still there afterwards must not get lost.*

```toml
[downtimegroup.patching]
hostgroups = ["linux"]
schedule = [{ days = ["sun"], from = "02:00", to = "04:00" }]

[[notify]]
contacts = ["oncall"]
repeat = "1h"
```

During the window, problems are held back. The catch: a problem that
started in the window had its PROBLEM held back, and nothing new happens
when the window ends. Without `repeat` or `after`, nobody would be told.
With `repeat = "1h"`, it is sent once it has been open for an hour. So
a server that broke at 02:30 is reported at 04:00, right after the window.

The rule also sends DOWNTIMESTART and DOWNTIMEEND messages. The end
message names the state at that moment. Use `on = ["problem",
"recovery"]` if you don't want them.

## Flapping services

*A service that keeps changing between OK and CRITICAL should not send a
message every few minutes.*

This needs no configuration. When a service starts flapping, Watcher
sends one FLAPPINGSTART and holds back its problems and recoveries. When it
calms down, it sends FLAPPINGSTOP. Reminders and escalations wait too.
Use `on = ["problem", "recovery"]` on a rule if its contacts don't want the
flapping messages.

## Checking your setup

Who gets notified about a service, and why:

```sh
watcher config explain web01/http
```

```
└─ notifications
   ├─ notify team: matched: all; escalates after 15m; repeats every 30m  notify.toml:1
   │  └─ contactgroup team                                  contacts.toml:5
   └─ notify teamlead: matched: all; escalates after 1h 30m  notify.toml:7
      └─ contact lead                                       contacts.toml:1
```

Whether a contact's channels work:

```sh
watcher notify test lead
```

What was actually sent: the **Notifications** panel on each host and
service page, or the [notification log](notifications.md#notification-log).

## Restarts

With a database, restarting Watcher changes nothing in these recipes.
Watcher reads from the notification log who already got which problem:

- Nothing is sent twice.
- A reminder or escalation that fell due while Watcher was stopped goes
  out right after the start, once.
- Contacts who got a problem before the restart still get its RECOVERY.

Without a database, open problems start their time again after a restart,
and escalations sent before it get no RECOVERY.
