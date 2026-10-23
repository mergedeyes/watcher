# Selectors

Which hosts or services a [notify rule](objects.md#notify) takes
(`hosts`, `services`).

- Every field present must match (AND).
- A list matches any of its values (OR).
- `{}` matches everything.
- Globs: `*` any text, `?` one character.

## Host selector

| Field | Matches |
|---|---|
| `name` | host name, glob(s) |
| `address` | address, glob(s) |
| `node` | node label(s) |
| `groups` | any of the host groups |
| `vars.<x>` | var value, or any of a list |
| `exclude` | host selector; its matches are removed |
| `expr` | [expression](#expressions) |

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
hosts = { address = "10.0.1.*" }
```

By host group:

```toml
hosts = { groups = ["prod"] }
```

Host group and name (AND):

```toml
hosts = { groups = ["prod"], name = "db*" }
```

By var:

```toml
hosts = { vars.os = ["debian", "ubuntu"] }
```

Except:

```toml
hosts = { groups = ["prod"], exclude = { name = "lab*" } }
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
services = ["web01/http", "db*/pgsql", "backup"]
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
services = { checks = ["http"], hosts = { groups = ["prod"] } }
```

## Expressions

For what the fields can't express, e.g. OR across fields.

```toml
hosts = { expr = 'host.groups contains "db" or host.name ~ "^pg"' }
```

| Operand | |
|---|---|
| `host.name`, `host.address`, `host.node` | text |
| `host.groups` | list |
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
services = { expr = 'service.check == "disk" and not host.groups contains "test"' }
```

## Checking a selector

Unknown hosts, groups, checks or nodes in a selector: warning.

```sh
watcher config explain notify dba-critical # what a rule matches
```
