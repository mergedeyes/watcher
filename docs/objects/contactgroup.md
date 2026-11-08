# contactgroup

A contact group is a set of [contacts](contact.md). [Notify rules](notify.md)
can send to a whole group with `contact_groups`, so you don't have to list
every person in every rule.

```toml
[contactgroup.dba]
title = "DBAs"
members = ["alice", "bob"]

[[notify]]
contact_groups = ["dba"]
services = { checks = ["pgsql"] }
```

Alice and Bob get notifications about every service that uses the check
`pgsql`.

## Fields

| Field | Default | Description |
|---|---|---|
| `title` | | Name shown in the web UI. |
| `members` | | Names of the contacts in the group. |

## Members

A contact joins a contact group in either of two ways, and both can be
mixed:

```toml
# On the group
[contactgroup.dba]
members = ["bob"]

# On the contact
[contact.alice]
email = "alice@example.com"
groups = ["dba"]
```

## Examples

### An on-call team

```toml
[contactgroup.oncall]
title = "On Call"
members = ["alice", "carol"]

[[notify]]
contact_groups = ["oncall"]
hosts = { groups = ["prod"] }
```

## See also

- [contact](contact.md)
- [notify](notify.md)
