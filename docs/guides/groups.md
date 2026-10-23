# Groups

| Kind | Holds | Does | Join from |
|---|---|---|---|
| `hostgroup` | hosts | gives its hosts services, service groups, host check, vars | `members`, host `groups` |
| `servicegroup` | services | a bundle of services for hosts and host groups | `services`, service `groups` |
| `contactgroup` | contacts | recipients of notify rules | `members`, contact `groups` |

All of them also group for status pages, filters and notify rules.

Fields: [reference](../reference/objects.md#hostgroup).

## Host groups

Listed on the host:

```toml
[hostgroup.databases]

[[host]]
name = "db01"
groups = ["databases"]
```

By name (`*`, `?`):

```toml
[hostgroup.web-servers]
title = "Web Servers"
members = ["web*", "shop01"]
```

What its hosts get:

```toml
[hostgroup.linux]
members = ["*"]
services = ["ssh"]
servicegroups = ["linux-basics"]
host_check = "ping"
vars.disks.root = { path = "/" }
```

The host wins over its host groups. Two host groups of a host setting
`host_check`, `graph` or a var differently: error.

## Service groups

Listed on the group:

```toml
[servicegroup.linux-basics]
title = "Linux Basics"
services = ["ping", "load", "disk_root"]
```

Listed on the service:

```toml
[service.http]
groups = ["webshop"]
```

Assigned to a host or host group, a service group gives it all its
services:

```toml
[[host]]
name = "web01"
servicegroups = ["linux-basics"]
```

A service group assigned to nothing only groups, e.g. a "Webshop" view
across web and database services:

```toml
[servicegroup.webshop]
title = "Webshop"
services = ["http", "pgsql"]
```

On a single host:

```toml
[[host]]
name = "db01"
  [host.service.pgsql]
  groups = ["webshop"]
```

## Contact groups

```toml
[contact.alice]
email = "alice@example.com"
groups = ["dba"]

[contact.bob]
email = "bob@example.com"

[contactgroup.dba]
title = "DBAs"
members = ["bob"]
```

Used in [notify rules](notifications.md): `contact_groups = ["dba"]`.

## Using groups

In selectors:

```toml
[[notify]]
contacts = ["oncall"]
hosts = { groups = ["dc1"] }

[[notify]]
contact_groups = ["dba"]
services = { groups = ["webshop"] }
```

Web UI: Current Status → Host Groups / Service Groups: status totals per
group, linked to the filtered lists.

Members and why:

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
