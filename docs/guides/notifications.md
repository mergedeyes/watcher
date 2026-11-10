# Notifications

Watcher sends notifications by email and webhook. Setting them up takes
three steps:

1. An SMTP server in `watcher.toml`, for email. See [SMTP](#smtp).
2. [Contacts](../objects/contact.md): who receives notifications, and where.
3. [Notify rules](../objects/notify.md): which contacts get which
   notifications.

```toml
# conf.d/contacts.toml
[contact.oncall]
email = "oncall@example.com"

[[notify]]
contacts = ["oncall"]
```

Then test it:

```sh
watcher notify test oncall
```

When a notification is sent, and when it isn't, is described in
[When notifications are sent](../objects/notify.md#when-notifications-are-sent).

## SMTP

The mail server is set in `watcher.toml`. Changes need a restart.

```toml
public_url = "https://watcher.example.com"   # used for links in messages
instance_name = "msp-hub"                    # subject: [Watcher msp-hub] ...

[smtp]
host = "mail.example.com"
port = 587
tls = "starttls"
username = "watcher"
password_file = "smtp.pass"
from = "Watcher <watcher@example.com>"
```

A local mail relay without login or encryption:

```toml
[smtp]
host = "localhost"
tls = "none"
from = "watcher@example.com"
```

All SMTP fields: [watcher.toml](../reference/watcher-toml.md#smtp).

## Email

The subject names the event, the host or service, and its state:

```
[Watcher] PROBLEM: web01/http is CRITICAL
[Watcher] PROBLEM: web01/http is still CRITICAL
[Watcher] RECOVERY: web01/http is OK
[Watcher] PROBLEM: web01 is DOWN
[Watcher] DOWNTIMESTART: web01 is UP
```

The body, as plain text:

```
web01/http is CRITICAL (was WARNING)

HTTP CRITICAL - Socket timeout

Host:     web01 (10.0.0.5)
Service:  http
State:    CRITICAL, confirmed after 3 attempts
Time:     2026-10-23T08:15:02Z
Details:  https://watcher.example.com/services/web01/http
```

The `Details` link is only there if `public_url` is set.

A repeated notification (a rule with [`repeat`](../objects/notify.md#repeat))
or an escalation (a rule with [`after`](../objects/notify.md#after)) says
how long the problem has been open, and since when:

```
web01/http is still CRITICAL (for 2h 10m)

HTTP CRITICAL - Socket timeout

Host:     web01 (10.0.0.5)
Service:  http
State:    CRITICAL
Since:    2026-10-23T08:15:02Z
Time:     2026-10-23T10:25:40Z
Details:  https://watcher.example.com/services/web01/http
```

| Event | Sent when |
|---|---|
| PROBLEM | A host or service has a confirmed (HARD) problem, or the problem got worse (WARNING → CRITICAL). With `repeat`: again while it is still open. With `after`: to the escalation contacts once it has been open that long. |
| RECOVERY | A confirmed problem is back to OK or UP. |
| FLAPPINGSTART, FLAPPINGSTOP | A service starts or stops flapping. |
| DOWNTIMESTART, DOWNTIMEEND | A downtime starts or ends. |

## Webhook

A contact with `webhook` gets each notification as a POST request with a
JSON body:

```json
{
  "kind": "problem",
  "host": "web01",
  "address": "10.0.0.5",
  "service": "http",
  "host_check": false,
  "state": "critical",
  "previous_state": "warning",
  "attempts": 3,
  "output": "HTTP CRITICAL - Socket timeout",
  "long_output": "",
  "time": "2026-10-23T08:15:02Z",
  "since": "2026-10-23T08:15:02Z",
  "repeat": false,
  "url": "https://watcher.example.com/services/web01/http",
  "instance": "msp-hub"
}
```

| Field | Description |
|---|---|
| `kind` | `problem`, `recovery`, `flappingstart`, `flappingstop`, `downtimestart`, `downtimeend`, or `test` (from `watcher notify test`). |
| `host`, `address` | The host. |
| `service` | The service. For host notifications, the host check. |
| `host_check` | `true`: the notification is about the host. Then `critical` and `unknown` mean DOWN, `ok` and `warning` mean UP. |
| `state` | `ok`, `warning`, `critical` or `unknown`. |
| `previous_state` | The state before. `null` when a problem was just confirmed after SOFT results. |
| `attempts` | The attempt that confirmed the state. |
| `output`, `long_output` | The check output. For downtimes: which downtime it is. |
| `time` | Time of the result, RFC 3339. |
| `since` | For problems: when the host or service changed to this state, RFC 3339. `null` for other kinds. |
| `repeat` | `true`: a problem sent again because it is still open (see [`repeat`](../objects/notify.md#repeat)), or escalated (see [`after`](../objects/notify.md#after)). |
| `url` | Link to the service page. `null` without `public_url`. |
| `instance` | `instance_name` from `watcher.toml`. |

## Delivery

A webhook counts as delivered when the server answers with a 2xx status.
On 429, 5xx or a network error, Watcher tries again after 1 second and
after 5 seconds. Temporary SMTP errors are retried the same way.

Notifications that are held back are written to the log with the reason:

```
no notification for web01/http: in downtime (hostgroup linux)
no notification for db01/replication: acknowledged
```

## Notification log

With a database, Watcher writes down every notification it sends: when,
to which contact, over which channel, and whether it arrived. The host
and service pages show the latest ones under **Notifications**, failed
ones with the reason:

```
Time                 Type             Status    Contact  Channel  Result
2026-11-10 11:30:02  PROBLEM again    CRITICAL  lead     email    SENT
2026-11-10 10:00:04  PROBLEM          CRITICAL  team     webhook  FAILED webhook: ... Connection refused
```

`again` marks a problem sent again: a [repeat](../objects/notify.md#repeat)
or an [escalation](../objects/notify.md#after).

The log also keeps repeats and escalations on time across restarts:
Watcher reads from it who already got an open problem. Without a
database, that is kept in memory only, and a restart starts the time
again.

Entries are kept for 90 days. To find failed deliveries, or everything a
contact got:

```sh
curl -H "Authorization: Bearer $T" '.../api/v1/notifications?failed=true&since=7d'
curl -H "Authorization: Bearer $T" '.../api/v1/notifications?contact=lead&since=24h'
```

The table `notifications` can also be read with SQL, see
[Data](data.md#tables-and-views).

## Who gets a notification

```sh
watcher config explain web01/http
```

```
└─ notifications
   └─ notify rule at conf.d/notify.toml:1: matched: host: group prod  notify.toml:1
      └─ contact oncall                                             contacts.toml:1
```

```sh
watcher config explain contact oncall
```

## Reminders for open problems

A rule with `repeat` sends a problem again while it is still open:

```toml
[[notify]]
contacts = ["oncall"]
repeat = "1h"
```

Acknowledge the problem to stop the reminders. Details:
[repeat](../objects/notify.md#repeat).

## Escalations

A rule with `after` steps in only when a problem lasts. Here the team lead
hears about problems that nobody acknowledged within 30 minutes:

```toml
[[notify]]
contacts = ["oncall"]

[[notify]]
contacts = ["teamlead"]
after = "30m"
```

Acknowledging the problem in time stops the escalation. Details:
[after](../objects/notify.md#after).

For complete setups (quiet start, escalation ladders, one team per
service, maintenance windows), see [Alerting recipes](alerting-recipes.md).

*Not yet:* more channels.
