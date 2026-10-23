# Hosts and roles

- **host:** something to monitor.
- **role:** the services a kind of host gets.
- **service:** a check on a host, id `host/service`.

Fields: [reference](../reference/objects.md).

## A host

```toml
[[host]]
name = "web01"
address = "10.0.0.5"          # default: the name
roles = ["linux", "web"]
tags = ["prod"]
```

## A role

```toml
[role.linux]
host_check = "ping"           # service giving the host UP/DOWN
interval = "1m"               # default for its services
service.ping = {}             # check "ping"
service.load = {}
service.root = { check = "disk", vars = { path = "/" } }
```

`service.<name> = {}` uses the check of the same name; `check = "..."`
another one. Service entries take every check field (`interval`,
`thresholds`, `vars`, ...).

## Assigning roles

Listed on the host:

```toml
[[host]]
name = "web01"
roles = ["linux"]
```

By selector on the role:

```toml
[role.web]
hosts = { tags = ["web"] }
service.http = {}
```

```toml
[role.all]
hosts = {}                    # every host
service.ping = {}
```

```toml
[role.db]
hosts = { name = "db*", exclude = { tags = ["test"] } }
service.pgsql = {}
```

Role selectors match `name`, `address`, `tags`, `node`, `vars`, not
`roles` or `groups`. All fields: [Selectors](../reference/selectors.md).

## Including roles

```toml
[role.base]
host_check = "ping"
service.ping = {}

[role.linux]
include = ["base"]
service.load = {}
```

The including role wins over the included one.

## Services on one host

New service:

```toml
[[host]]
name = "db01"
roles = ["linux"]
  [host.service.backup_age]
  command = "/opt/scripts/backup_age.sh"
  interval = "1h"
```

Existing check:

```toml
  [host.service.pgsql]
  check = "pgsql"
```

Override a role's service:

```toml
  [host.service.load]
  thresholds.load1 = { warn = "> 16" }
```

Remove it:

```toml
  [host.service.load]
  enabled = false
```

## Vars

Values for `$name$` in commands.

```toml
[check.http]
command = "check_http -H $host.address$ -u $url$"
vars.url = "/"

[[host]]
name = "shop"
roles = ["web"]
vars.url = "/health"          # → check_http -H shop -u /health
```

Lookup, most specific first:

1. host service (`[host.service.x] vars`)
2. `for_each` entry
3. host `vars`
4. role service entry `vars`
5. role `vars`
6. check `vars`

## Precedence

host > role > included role > check > defaults, key by key.

- Tables (`vars`, `thresholds`) merge per key.
- Lists are replaced; `tags` and `groups` add up.
- Two roles on one host setting the same service, var, `host_check`,
  `graph` or `node` differently: **error**. Fix it on the host, or in a
  role including both.

```
hosts.toml:8: error: host "h1": service "ping" comes from roles "a" and "b"
with different settings; override it on the host or in a role including both
```

## One service per item: for_each

One service per entry of a host var. Name: `<service>_<key>`.

```toml
[check.disk]
command = "check_disk -w $warn$ -c $crit$ -p $path$"
vars = { warn = "20%", crit = "10%" }

[role.linux]
service.disk = { for_each = "disks" }
vars.disks.root = { path = "/" }                              # every linux host

[[host]]
name = "db01"
roles = ["linux"]
vars.disks.data = { path = "/srv", warn = "10%", crit = "5%" }
```

```
db01/disk_data   check_disk -w '10%' -c '5%' -p /srv
db01/disk_root   check_disk -w '20%' -c '10%' -p /
```

Override one: `[host.service.disk_root]`. All: `[host.service.disk]`.

## Host state

`host_check` names a service on the host:

| Service | Host |
|---|---|
| OK, WARNING | UP |
| CRITICAL, UNKNOWN | DOWN |

- No host check: always UP.
- Host DOWN (HARD): its other services don't notify.
- The host check notifies as the host: "web01 is DOWN".

## Graph

The host page graphs `graph` (a service name), default the host check.

```toml
[role.linux]
graph = "load"
```

## Disabling a host

```toml
[[host]]
name = "old01"
enabled = false
```

Removed hosts and services keep their history in the database.

## Where does it come from?

```sh
watcher config explain db01/disk_data
```

```
db01/disk_data                                              hosts.toml:5
├─ from
│  ├─ host db01                                             hosts.toml:1
│  │  └─ vars.disks.data: for_each entry                    hosts.toml:1
│  ├─ role linux: listed on host db01                       hosts.toml:1
│  │  └─ role linux, service disk                           roles.toml:3
│  └─ check disk                                            checks.toml:1
│     └─ command: check_disk -w $warn$ -c $crit$ -p $path$
└─ values
   ├─ command: check_disk -w '10%' -c '5%' -p /srv          checks.toml:1
   │  └─ check disk                                         checks.toml:1
   ├─ interval: 5m                                          hosts.toml:5
   │  └─ host db01, service disk                            hosts.toml:5
   ├─ retry_interval: 1m
   │  └─ default
   ...
   └─ vars.warn: 10%                                        hosts.toml:1
      └─ host db01, vars.disks.data                         hosts.toml:1
```

Web UI: **Config Tree** on host, service and group pages and every config
form.

```sh
watcher config explain role linux
watcher config check --services     # every service with its command
```
