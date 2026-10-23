# Selectors

Which hosts or services. Used by role `hosts`, group `members` and notify
rules.

- Every field present must match (AND).
- A list matches any of its values (OR).
- `{}` matches everything.
- Globs: `*` any text, `?` one character.

## Host selector

| Field | Matches |
|---|---|
| `name` | host name, glob(s) |
| `address` | address, glob(s) |
| `tags` | any of the tags |
| `node` | node label(s) |
| `roles` | any of the roles |
| `groups` | any of the host groups |
| `vars.<x>` | var value, or any of a list |
| `exclude` | host selector; its matches are removed |
| `expr` | [expression](#expressions) |

Not allowed: role `hosts`: `roles`, `groups` (they depend on roles).
Host group `members`: `groups`.

By name:

```toml
hosts = { name = "web*" }
```

Shorthand, list of names:

```toml
hosts = ["web01", "db*"]
```

By address:

```toml
members = { address = "10.0.1.*" }
```

By tag:

```toml
hosts = { tags = ["prod"] }
```

Tag and name (AND):

```toml
hosts = { tags = ["prod"], name = "db*" }
```

By var:

```toml
hosts = { vars.os = ["debian", "ubuntu"] }
```

By role (not in role `hosts`):

```toml
hosts = { roles = ["db"] }
```

By host group (not in host group `members`):

```toml
hosts = { groups = ["dc1"] }
```

Except:

```toml
hosts = { tags = ["prod"], exclude = { name = "lab*" } }
```

All:

```toml
hosts = {}
```

## Service selector

| Field | Matches |
|---|---|
| `name` | service name, glob(s) |
| `checks` | any of the checks |
| `groups` | any of the service groups |
| `hosts` | [host selector](#host-selector) |
| `exclude` | service selector; its matches are removed |
| `expr` | [expression](#expressions) |

Shorthand, list of `host/service` ids or service names, globs allowed:

```toml
members = ["web01/http", "db*/pgsql", "backup"]
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

On some hosts:

```toml
services = { checks = ["http"], hosts = { tags = ["prod"] } }
```

## Expressions

For what the fields can't express, e.g. OR across fields.

```toml
hosts = { expr = 'host.tags contains "db" or host.name ~ "^pg"' }
```

| Operand | |
|---|---|
| `host.name`, `host.address`, `host.node` | text |
| `host.tags`, `host.roles`, `host.groups` | lists |
| `host.vars.<x>` | var |
| `service.name`, `service.check`, `service.groups` | services only |

| Operator | |
|---|---|
| `==`, `!=` | equal |
| `~`, `!~` | regex match |
| `contains` | list contains |
| `in` | value in `[...]` |
| `and` / `&&`, `or` / `\|\|`, `not` / `!` | logic |
| `( )` | grouping |

Strings in `"..."` or `'...'`. Unset vars never match (`!=` on an unset
var is true).

```toml
hosts = { expr = 'host.vars.os == "debian" && host.address ~ "^10\.0\."' }
```

```toml
hosts = { expr = 'host.name in ["db01", "web01"]' }
```

```toml
services = { expr = 'service.check == "disk" and not host.tags contains "test"' }
```

## Checking a selector

Unknown tags, roles, groups or nodes in a selector: warning. Empty match
for a role or group: warning.

```sh
watcher config explain hostgroup dc1       # members and why
watcher config explain notify dba-critical # what a rule matches
```
