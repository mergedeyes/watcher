# Notifications

Channels: email, webhook. Who gets what: `[[notify]]` rules.

## When

| Type | Sent on |
|---|---|
| PROBLEM | HARD change to a problem state, incl. WARNING → CRITICAL |
| RECOVERY | HARD change from a problem back to OK |
| FLAPPINGSTART, FLAPPINGSTOP | flapping starts / stops |

Not sent:

- SOFT states
- first OK results, states restored after a restart
- other services of a host that is HARD DOWN
- problems while flapping
- host check WARNING (host stays UP), CRITICAL ↔ UNKNOWN on a host check

The host check notifies as the host: `web01 is DOWN`.

*Not yet:* re-notification, acknowledgements, downtimes, escalations.

## SMTP

In `watcher.toml`:

```toml
public_url = "https://watcher.example.com"   # links in messages
instance_name = "msp-hub"                    # subject: [Watcher msp-hub]

[smtp]
host = "mail.example.com"
port = 587                    # default: 587 starttls, 465 tls, 25 none
tls = "starttls"              # starttls | tls | none
username = "watcher"
password_file = "smtp.pass"   # or: password = "..."
from = "Watcher <watcher@example.com>"
```

Local relay:

```toml
[smtp]
host = "localhost"
tls = "none"
from = "watcher@example.com"
```

Subject: `[Watcher] PROBLEM: web01/http is CRITICAL`.

## Contacts

```toml
[contact.oncall]
email = "oncall@example.com"

[contact.alice]
email = ["alice@example.com", "alice@pager.example.com"]

[contact.chatops]
webhook = "https://hooks.example.com/watcher"
```

Test:

```sh
watcher notify test oncall
```

## Notify rules

Every rule that matches adds its contacts. Each contact gets one message.
No order, no "stop".

Everything:

```toml
[[notify]]
contacts = ["oncall"]
```

By host:

```toml
[[notify]]
contacts = ["oncall"]
hosts = { groups = ["prod"] }
```

By service:

```toml
[[notify]]
contact_groups = ["dba"]
services = { checks = ["pgsql"] }
```

Only some states:

```toml
[[notify]]
name = "critical-only"
contacts = ["manager"]
hosts = { groups = ["dc1"] }
states = ["critical", "down"]
```

Only some types:

```toml
[[notify]]
contacts = ["chatops"]
on = ["problem"]              # no recoveries, no flapping
```

Except:

```toml
[[notify]]
contacts = ["oncall"]
hosts = { groups = ["prod"], exclude = { name = "lab*" } }
```

Off:

```toml
[[notify]]
contacts = ["oncall"]
enabled = false
```

| Field | Default | |
|---|---|---|
| `name` | | for the UI and Config Tree |
| `contacts`, `contact_groups` | | who |
| `hosts` | all | [host selector](../reference/selectors.md#host-selector) |
| `services` | all | [service selector](../reference/selectors.md#service-selector) |
| `states` | all | `warning`, `critical`, `unknown`, `down` |
| `on` | all | `problem`, `recovery`, `flapping` |
| `enabled` | `true` | |

`hosts` and `services` both set: both must match.

A recovery reaches a rule only if the state it recovers from is in the
rule's `states`: a rule with `states = ["critical"]` gets CRITICAL → OK, not
WARNING → OK.

Unnamed rules can't be edited in the web UI; give them a `name`.

## Who gets an alert

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

## Webhook

POST, JSON:

```json
{
  "kind": "problem",
  "host": "web01",
  "address": "10.0.0.5",
  "service": "http",
  "host_check": false,
  "state": "critical",
  "previous_state": null,
  "attempts": 3,
  "output": "HTTP CRITICAL - Socket timeout",
  "long_output": "",
  "time": "2026-10-23T08:15:02Z",
  "url": "https://watcher.example.com/services/web01/http",
  "instance": "msp-hub"
}
```

| Field | |
|---|---|
| `kind` | `problem`, `recovery`, `flappingstart`, `flappingstop`, `test` |
| `host_check` | `true`: about the host; `critical`/`unknown` = DOWN |
| `previous_state` | the state before; `null` when a SOFT problem was just confirmed |
| `url` | `null` without `public_url` |

2xx: delivered. 429, 5xx, network errors: retried after 1 s and 5 s. Same
for temporary SMTP errors.
