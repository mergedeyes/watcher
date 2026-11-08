# contact

A contact is someone who receives notifications, by email, webhook, or
both. A contact alone receives nothing: a [notify rule](notify.md) decides
which contacts get which notifications.

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
| `groups` | | Contact groups the contact belongs to. See [contactgroup](contactgroup.md). |

A contact without `email` and `webhook` gives a warning.

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
- [Notifications](../guides/notifications.md): SMTP, message format, webhooks
