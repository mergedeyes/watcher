# Operations

Day-to-day actions on hosts and services: acknowledging problems,
downtimes, muting notifications and turning checks off. These are stored in
the database (in memory when there is no database), not in the config
files. The exception is recurring downtimes, which are
[downtime groups](../objects/downtimegroup.md) in the config.

In the web UI, right-click a host or service, or use the Commands panel on
its page. Downtimes are listed under System → Downtimes. Over the API, use
[`POST actions/{action}`](../reference/api.md#commands).

| Command | What it does | Ends when |
|---|---|---|
| Re-Check Now | Runs the check now. | – |
| Acknowledge | Stops problem notifications; shows the badge ACK. | the service is OK again, or you remove it |
| Schedule Downtime | No notifications during the window; shows the badge DOWNTIME. | the end time is reached, or you end it |
| Mute Notifications | No notifications; shows the badge MUTED. | you unmute |
| Disable Active Checks | The check doesn't run any more; shows the badge CHECKS OFF. | you enable it |
| Submit Result | Enters a result for a passive service. | – |

On a host, Acknowledge applies to its host check (the host is DOWN). All
other commands apply to all its services.

The recovery after an acknowledged problem is still notified; it also ends
the acknowledgement.

Notifications that are held back are written to the log with the reason:

```
no notification for web01/http: in downtime (hostgroup linux)
no notification for db01/replication: acknowledged
```

## Acknowledging a problem

Acknowledging tells the team that someone is working on a problem. It
stops further problem notifications until the service is OK again,
including the reminders of rules with
[`repeat`](../objects/notify.md#repeat) and the escalations of rules with
[`after`](../objects/notify.md#after). Add a comment so others know what
is going on.

```sh
curl -X POST -H "Authorization: Bearer $T" -H 'content-type: application/json' \
  -d '{"services": ["db01/replication"], "comment": "on it"}' \
  .../api/v1/actions/ack
```

## Downtimes

A downtime is a planned window without notifications, for example for
maintenance. One-off downtimes can be scheduled for hosts, services, host
groups, service groups and downtime groups:

```sh
curl -X POST -H "Authorization: Bearer $T" -H 'content-type: application/json' \
  -d '{"hostgroups": ["linux"], "start": "2026-11-01T22:00:00+01:00", "duration": "4h", "comment": "kernel updates"}' \
  .../api/v1/actions/downtime
```

A downtime for a group also covers hosts that join the group after it was
scheduled.

For recurring windows, like a weekly patch day, use a
[downtime group](../objects/downtimegroup.md).

When a downtime starts or ends, the notify rules that match the host or
service are notified (DOWNTIMESTART, DOWNTIMEEND), unless the rule leaves
`downtime` out of `on` or the host or service is muted:

```
[Watcher] DOWNTIMESTART: web01 is UP
web01 is in a downtime (UP); no notifications until it ends

hostgroup linux: kernel updates
```

A host in a downtime is notified once, not once per service. Watcher
notices downtimes starting or ending within 15 seconds.

## History

The Alerts page and the host and service pages list acknowledgements (ACK,
with author and comment) and the start and end of downtimes. Filter by
Alert type Acknowledgement or Downtime, or over the API with
`events?type=ack,downtime`.

## Turning everything off

System → Process Info has two global switches, for admins: notifications on
or off, and active checks on or off, for all hosts and services at once.

```sh
curl -X PUT -H "Authorization: Bearer $T" -H 'content-type: application/json' \
  -d '{"notifications": false}' .../api/v1/process
```

## Process info and scheduling queue

| Page | Shows |
|---|---|
| Process Info | Version, PID, running time, when the config was loaded, results in the last 1, 5 and 15 minutes, and how many hosts and services are acknowledged, muted, have checks off, or are in a downtime. |
| Scheduling Queue | Every service with its last check, next check and interval, and whether its checks are off. |
