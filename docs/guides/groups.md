# Groups

Groups only group: for status pages, filters, selectors and notify rules.
They don't assign services (roles do) and don't pass on settings.

| Kind | Groups | Join from |
|---|---|---|
| `hostgroup` | hosts | `members`, host `groups`, role `groups` |
| `servicegroup` | services | `members`, service `groups` |
| `contactgroup` | contacts | `members`, contact `groups` |

Fields: [reference](../reference/objects.md#hostgroup).

## Host groups

By selector:

```toml
[hostgroup.dc1]
title = "Datacenter 1"
members = { address = "10.0.*" }
```

Listed on the host:

```toml
[hostgroup.databases]

[[host]]
name = "db01"
groups = ["databases"]
```

Every host of a role:

```toml
[hostgroup.web-servers]
title = "Web Servers"

[role.web]
hosts = { tags = ["web"] }
groups = ["web-servers"]
service.http = {}
```

Host group `members` can't use `groups`.

## Service groups

Listed on the service:

```toml
[servicegroup.webshop]
title = "Webshop"

[role.web]
service.http = { groups = ["webshop"] }

[[host]]
name = "db01"
  [host.service.pgsql]
  groups = ["webshop"]
```

By id:

```toml
[servicegroup.payment]
members = ["db01/pgsql", "web*/http"]
```

By selector:

```toml
[servicegroup.http]
members = { checks = ["http"] }
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

Why a member is in:

```sh
watcher config explain servicegroup webshop
```

```
Webshop                                                     groups.toml:1
└─ members
   ├─ service web01/http: listed on role web, service http  roles.toml:4
   └─ service db01/pgsql: listed on host db01, service pgsql  hosts.toml:9
```
