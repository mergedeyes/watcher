# contact

A contact is someone who receives notifications: by email, webhook,
Slack, Microsoft Teams, Telegram, ntfy, Gotify, PagerDuty or a script of
your own, or several of them. A contact alone receives nothing: a
[notify rule](notify.md) decides which contacts get which notifications.

```toml
[contact.oncall]
email = "oncall@example.com"

[[notify]]
contacts = ["oncall"]
```

The contact `oncall` gets an email for every problem and recovery.

Email needs an SMTP server in `watcher.toml`, see
[Notifications](../guides/notifications.md#smtp).

## Fields

| Field | Default | Description |
|---|---|---|
| `email` | | One address, or a list of addresses. |
| `webhook` | | An `http://` or `https://` URL. Watcher sends each notification as JSON in a POST request; see [Webhook](../guides/notifications.md#webhook). |
| `slack` | | A Slack incoming webhook URL. See [Slack](../guides/notifications.md#slack). |
| `teams` | | A Microsoft Teams webhook URL, from the Workflows template "Post to a channel when a webhook request is received". See [Microsoft Teams](../guides/notifications.md#microsoft-teams). |
| `telegram.token` | | Telegram: the bot's token from BotFather. See [Telegram](../guides/notifications.md#telegram). |
| `telegram.chat` | | Telegram: the chat ID (a number, `-100…` for groups) or `@channelname`. |
| `ntfy` | | An ntfy topic URL, `https://ntfy.sh/my-topic`. Or a table `{ url, token }` for a topic that needs an access token. See [ntfy](../guides/notifications.md#ntfy). |
| `gotify.url` | | Gotify: the server's URL. See [Gotify](../guides/notifications.md#gotify). |
| `gotify.token` | | Gotify: an application token. |
| `pagerduty` | | A PagerDuty Events API v2 routing key (integration key). Problems open incidents, recoveries resolve them. See [PagerDuty](../guides/notifications.md#pagerduty). |
| `script` | | A program to run for each notification, with arguments, for example `/usr/local/bin/send-sms --to ops`. It gets the notification as JSON on stdin. See [Scripts](../guides/notifications.md#scripts). |
| `groups` | | Contact groups the contact belongs to. See [contactgroup](contactgroup.md). |

A contact without any of `email`, `webhook` and the other channels gives a
warning.

## Examples

### One email address

```toml
[contact.alice]
email = "alice@example.com"
```

### Several addresses

```toml
[contact.alice]
email = ["alice@example.com", "alice@pager.example.com"]
```

### A chat webhook

```toml
[contact.chatops]
webhook = "https://hooks.example.com/watcher"
```

### Email and webhook

The contact gets each notification on both channels.

```toml
[contact.noc]
email = "noc@example.com"
webhook = "https://chat.example.com/hooks/watcher"
```

### Slack and Microsoft Teams

```toml
[contact.team-chat]
slack = "https://hooks.slack.com/services/T0000/B0000/XXXXXXXX"
teams = "https://prod-00.westeurope.logic.azure.com/workflows/…"
```

### Telegram

```toml
[contact.oncall-phone]
telegram = { token = "123456789:AAH…", chat = -1001234567890 }
```

### Push to a phone with ntfy or Gotify

```toml
[contact.alice]
ntfy = "https://ntfy.sh/acme-ops-7f3k2"

[contact.bob]
ntfy = { url = "https://ntfy.example.com/ops", token = "tk_…" }
gotify = { url = "https://gotify.example.com", token = "AbCdEf…" }
```

### PagerDuty

```toml
[contact.pagerduty]
pagerduty = "R0123456789ABCDEF0123456789ABCD"
```

### A script

```toml
[contact.sms]
script = "/usr/local/bin/send-sms --to +491701234567"
```

### In a contact group

```toml
[contact.alice]
email = "alice@example.com"
groups = ["dba"]
```

## Testing

```sh
watcher notify test oncall
```

This sends a test message to the contact on all its channels.

To see which notifications a contact gets:

```sh
watcher config explain contact oncall
```

## See also

- [notify](notify.md): who gets which notifications
- [contactgroup](contactgroup.md)
- [Notifications](../guides/notifications.md): SMTP, message format, webhooks, setting up each channel
