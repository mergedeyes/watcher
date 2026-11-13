# servicegroup

A service group is a set of services. It has two uses:

- **Bundle services** for hosts: a host or host group that lists the group
  gets all its services.
- **Group services** for the status pages and [notify rules](notify.md),
  for example a "Webshop" view of web and database services.

```toml
[servicegroup.linux-basics]
title = "Linux Basics"
services = ["ping", "load", "disk_root"]

[hostgroup.linux]
members = ["*"]
servicegroups = ["linux-basics"]
```

Every host gets `ping`, `load` and `disk_root`.

## Fields

| Field | Default | Description |
|---|---|---|
| `title` | | Name shown in the web UI. |
| `services` | | Names of the services in the group. A [built-in check](check.md#built-in-checks) like `http` or `tls` can be listed as it is, without a `[service.<name>]`. |

## Members

A service joins a service group in either of two ways, and both can be
mixed:

```toml
# On the group
[servicegroup.webshop]
services = ["http", "pgsql"]

# On the service
[service.redis]
groups = ["webshop"]
```

On one host only, with a [host service](host.md#host-services):

```toml
[[host]]
name = "db01"
  [host.service.pgsql]
  groups = ["webshop"]
```

## Examples

### A bundle for every server

```toml
[servicegroup.server]
title = "Server Basics"
services = ["ping", "ssh", "load"]

[hostgroup.all]
members = ["*"]
servicegroups = ["server"]
```

### A bundle for one host

```toml
[[host]]
name = "web01"
servicegroups = ["linux-basics"]
```

### A view across hosts

This group is not assigned to any host. It only groups services that the
hosts already have, so they show together on the status page and can be
used in notify rules.

```toml
[servicegroup.webshop]
title = "Webshop"
services = ["http", "pgsql"]
```

### Notifications for the group

```toml
[[notify]]
contact_groups = ["shop-team"]
services = { groups = ["webshop"] }
```

## Checking members

```sh
watcher config explain servicegroup linux-basics
```

```
linux-basics                                                groups.toml:5
├─ services
│  ├─ service ping: listed                                  services.toml:1
│  └─ service load: listed                                  services.toml:2
├─ assigned to
│  └─ hostgroup linux                                       groups.toml:1
└─ members
   ├─ service web01/ping: listed on servicegroup linux-basics  groups.toml:5
   └─ service web01/load: listed on servicegroup linux-basics  groups.toml:5
```

In the web UI, Current Status → Service Groups shows the status totals of
each group.

## See also

- [service](service.md)
- [hostgroup](hostgroup.md)
- [How a host gets its services](../guides/configuration.md#how-a-host-gets-its-services)
