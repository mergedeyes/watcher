# Notifications

Watcher sends notifications by email, webhook, Slack, Microsoft Teams,
Telegram, ntfy, Gotify, PagerDuty or a script of your own. Setting them
up takes three steps:

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

## Chat, push and paging services

Besides email and webhooks, a [contact](../objects/contact.md) can get
notifications on Slack, Microsoft Teams, Telegram, ntfy, Gotify and
PagerDuty, or through a script. Each service gets the same text as an
email: what happened, the check output, host, service, state, time and
the link to Watcher. Colours and priorities follow the state: CRITICAL
and DOWN are red or urgent, recoveries green.

A contact can have several channels; it gets each notification on all of
them. Test them with `watcher notify test <contact>`.

### Slack

1. Create a Slack app at [api.slack.com/apps](https://api.slack.com/apps)
   (From scratch), or open an existing one.
2. Under **Incoming Webhooks**, turn them on and click **Add New Webhook**.
3. Pick the channel and copy the webhook URL.
4. Put it on a contact:

```toml
[contact.ops-slack]
slack = "https://hooks.slack.com/services/T0000/B0000/XXXXXXXX"
```

### Microsoft Teams

1. In Teams, open the channel, then **⋯ → Workflows**.
2. Choose the template **Post to a channel when a webhook request is
   received**, pick the team and channel, and finish the setup.
3. Copy the URL the workflow shows.
4. Put it on a contact:

```toml
[contact.ops-teams]
teams = "https://prod-00.westeurope.logic.azure.com/workflows/…"
```

Watcher sends an Adaptive Card. Old Office 365 connector URLs work too.

### Telegram

1. In Telegram, write to **@BotFather**, send `/newbot` and follow the
   steps. You get a token like `123456789:AAH…`.
2. Add the bot to the group that should get the notifications, or start
   a chat with the bot yourself.
3. Send a message in that chat, then open
   `https://api.telegram.org/bot<token>/getUpdates` in a browser. The
   chat ID is `"chat":{"id":…}`; group IDs are negative.
4. Put both on a contact:

```toml
[contact.oncall-telegram]
telegram = { token = "123456789:AAH…", chat = -1001234567890 }
```

For a public channel, `chat = "@channelname"` works too; the bot must be
an admin of the channel.

### ntfy

[ntfy](https://ntfy.sh) sends push notifications to phones and desktops.

1. Choose a topic. On the public server `ntfy.sh` anyone who knows the
   topic name can read it, so use a name that is hard to guess, or your
   own server.
2. Subscribe to the topic in the ntfy app.
3. Put the topic URL on a contact:

```toml
[contact.alice-phone]
ntfy = "https://ntfy.sh/acme-ops-7f3k2"
```

For a topic that needs a login, use an access token:

```toml
[contact.alice-phone]
ntfy = { url = "https://ntfy.example.com/ops", token = "tk_…" }
```

Problems are sent with high priority, CRITICAL and DOWN with ntfy's
highest (urgent); recoveries with the default. Tapping the notification
opens Watcher.

### Gotify

1. In the Gotify web UI, open **Apps** and create an application.
2. Copy its token.
3. Put the server and token on a contact:

```toml
[contact.ops-gotify]
gotify = { url = "https://gotify.example.com", token = "AbCdEf…" }
```

### PagerDuty

1. In PagerDuty, open the service, then **Integrations → Add an
   integration → Events API V2**.
2. Copy the **Integration Key**.
3. Put it on a contact:

```toml
[contact.pagerduty]
pagerduty = "R0123456789ABCDEF0123456789ABCD"
```

A PROBLEM opens an incident (severity critical, warning or error), and
the RECOVERY of the same host or service resolves it. Repeats and
escalations update the open incident instead of opening a new one.
Flapping and downtime messages are not sent to PagerDuty. `watcher notify
test` opens a test incident and resolves it right away.

*Not yet:* acknowledging in PagerDuty doesn't acknowledge in Watcher.

### Scripts

For anything else, for example SMS through a modem or a gateway, a
contact can run a program:

```toml
[contact.sms]
script = "/usr/local/bin/send-sms --to +491701234567"
```

The program is started directly, without a shell; arguments are split
like in check commands, so quotes work. It gets the notification as JSON
on stdin (the same as a [webhook](#webhook) body) and the main values in
environment variables:

| Variable | Value |
|---|---|
| `WATCHER_KIND` | `PROBLEM`, `RECOVERY`, `FLAPPINGSTART`, `FLAPPINGSTOP`, `DOWNTIMESTART`, `DOWNTIMEEND` or `TEST` |
| `WATCHER_HOST`, `WATCHER_SERVICE` | The host and service; for host notifications, the service is the host check |
| `WATCHER_HOST_CHECK` | `true` for host notifications |
| `WATCHER_STATE` | `OK`, `WARNING`, `CRITICAL`, `UNKNOWN`, `UP` or `DOWN` |
| `WATCHER_REPEAT` | `true` if the problem is sent again (repeat or escalation) |
| `WATCHER_SUBJECT` | The email subject: `[Watcher] PROBLEM: web01/http is CRITICAL` |
| `WATCHER_SUMMARY` | One line: `web01/http is CRITICAL (was OK)` |
| `WATCHER_OUTPUT` | The check output |
| `WATCHER_URL` | The link to Watcher, empty without `public_url` |
| `WATCHER_CONTACT` | The contact's name |

Exit code 0 means delivered. Any other exit code is a failure; the first
line the script wrote to stderr shows up in the log and the
[notification log](#notification-log). A script may run for 30 seconds
and is not retried: it can retry itself. It runs as the user Watcher runs
as.

## Delivery

Webhooks and the chat, push and paging services count as delivered when
the service answers with a 2xx status. On 429, 5xx or a network error,
Watcher tries again after 1 second and after 5 seconds. Temporary SMTP
errors are retried the same way. When a service refuses a message, its
answer is kept in the error, for example
`telegram: answered 400 Bad Request: … chat not found`.

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

## Business hours and on-call

A rule with `period` only notifies inside a [time period](../objects/timeperiod.md).
Problems that are still open when it begins are sent then:

```toml
[timeperiod.workhours]
schedule = [{ days = ["mon", "tue", "wed", "thu", "fri"], from = "08:00", to = "18:00" }]

[[notify]]
contact_groups = ["team"]
period = "workhours"
```

Details: [period](../objects/notify.md#period).

For complete setups (quiet start, escalation ladders, on-call at night,
one team per service, maintenance windows), see
[Alerting recipes](alerting-recipes.md).
