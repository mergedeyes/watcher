# Operations

Acknowledgements, downtimes, muted notifications and disabled checks.
Kept in the database (in memory without one), not in the config files;
recurring downtimes are [downtime groups](#downtime-groups) in the config.

Web UI: right-click a host or service, or its Commands panel; System →
Downtimes. API: [`POST actions/{action}`](../reference/api.md#commands).

| Command | Effect | Ends |
|---|---|---|
| Re-Check Now | runs the check now | – |
| Acknowledge | no more problem notifications; badge ACK | OK again, or Remove |
| Schedule Downtime | no notifications in the window; badge DOWNTIME | end time, or End |
| Mute Notifications | no notifications; badge MUTED | Unmute |
| Disable Active Checks | checks don't run; badge CHECKS OFF | Enable |
| Submit Result | a result for a passive service | – |

On a host: Acknowledge covers its host check (host DOWN); the others
cover all its services.

Held-back notifications are logged with the reason:

```
no notification for web01/http: in downtime (hostgroup linux)
no notification for db01/replication: acknowledged
```

Recoveries still notify after an acknowledgement (it ends with them).

## History and notifications

Alerts (and the host and service pages) list acknowledgements (ACK, with
author and comment) and downtimes starting and ending. Filter: Alert type
Acknowledgement / Downtime; API `events?type=ack,downtime`.

A downtime starting or ending notifies like Nagios (DOWNTIMESTART,
DOWNTIMEEND) to the notify rules that match the host or service, unless
they leave `downtime` out of `on`, or it is muted:

```
[Watcher] DOWNTIMESTART: web01 is UP
web01 is in a downtime (UP); no notifications until it ends

hostgroup linux: kernel updates
```

A host in a downtime counts once, not per service. Changes are noticed
within 15 s.

## Downtimes

One-off, for hosts, services, host groups, service groups or downtime
groups:

```sh
curl -X POST -H "Authorization: Bearer $T" -H 'content-type: application/json' \
  -d '{"hostgroups": ["linux"], "start": "2026-11-01T22:00:00+01:00", "duration": "4h", "comment": "kernel updates"}' \
  .../api/v1/actions/downtime
```

A downtime for a group covers hosts that join it later.

## Downtime groups

Hosts that share downtimes, with recurring windows in local time
(`timezone` in [watcher.toml](../reference/watcher-toml.md), default the
system's).

```toml
[downtimegroup.patchday]
title = "Patch Day"
members = ["web*", "db01"]                 # host names, * and ?
hostgroups = ["linux"]
schedule = [
  { days = ["sun"], from = "02:00", to = "04:00" },
  { days = ["sat"], from = "22:00", to = "02:00" },          # into Sunday
  { from = "2026-12-24 00:00", to = "2026-12-27 00:00" },    # once
]
```

| Window | |
|---|---|
| `days` | `mon` … `sun`; none: every day |
| `from`, `to` | `HH:MM`; `to` not after `from`: next day |
| dates | `YYYY-MM-DD HH:MM` in `from` and `to`: once |

Also a target for one-off downtimes: Schedule Downtime → Downtime Group.
System → Downtimes shows the current or next window of each.

## Global switches

System → Process Info (admins): notifications and active checks on/off
for everything.

```sh
curl -X PUT -H "Authorization: Bearer $T" -H 'content-type: application/json' \
  -d '{"notifications": false}' .../api/v1/process
```

## Process info and queue

| Page | |
|---|---|
| Process Info | version, PID, running time, config loaded, results in the last 1/5/15 min, counts of acknowledged, muted, checks off, in downtime |
| Scheduling Queue | every service: last check, next check, interval, CHECKS OFF |
