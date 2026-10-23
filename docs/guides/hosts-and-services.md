# Hosts and services

- **check:** what to run and how to judge it.
- **service:** a check with its settings, defined once.
- **host:** something to monitor.
- **service on a host:** id `host/service`.
- **host group**, **service group:** tie them together.

```
check ─▶ service ─▶ host
            │         ▲
            ▼         │
     servicegroup ─▶ hostgroup
```

Fields: [reference](../reference/objects.md).

## 1. A check

```toml
[check.ping]
command = "check_ping -H $host.address$ -w $warn$ -c $crit$"
vars = { warn = "100,20%", crit = "500,60%" }
```

[Checks](checks.md).

## 2. A service

```toml
[service.ping]                # check "ping"

[service.disk_root]
check = "disk"
vars.path = "/"
interval = "5m"
```

Services take every check field (`interval`, `thresholds`, `vars`, ...);
`check` defaults to the check of the same name.

## 3. A host

```toml
[[host]]
name = "web01"
address = "10.0.0.5"          # default: the name
```

## 4. Assigning services

On the host:

```toml
[[host]]
name = "web01"
services = ["ping", "disk_root"]
```

In a service group:

```toml
[servicegroup.linux-basics]
services = ["ping", "load", "disk_root"]

[[host]]
name = "web01"
servicegroups = ["linux-basics"]
```

Through a host group:

```toml
[hostgroup.linux]
members = ["web*", "db01"]    # or groups = ["linux"] on the host
servicegroups = ["linux-basics"]
services = ["ssh"]
host_check = "ping"           # service giving its hosts UP/DOWN
```

Every host:

```toml
[hostgroup.all]
members = ["*"]
services = ["ping"]
```

A service reached several ways is there once.

## Services on one host

Override an assigned service:

```toml
[[host]]
name = "db01"
groups = ["linux"]
  [host.service.load]
  thresholds.load1 = { warn = "> 16" }
```

Remove it:

```toml
  [host.service.load]
  enabled = false
```

One-off service:

```toml
  [host.service.backup_age]
  command = "/opt/scripts/backup_age.sh"
  interval = "1h"
```

## Vars

Values for `$name$` in commands.

```toml
[check.http]
command = "check_http -H $host.address$ -u $url$"
vars.url = "/"

[[host]]
name = "shop"
services = ["http"]
vars.url = "/health"          # → check_http -H shop -u /health
```

Lookup, most specific first:

1. host service (`[host.service.x] vars`)
2. `for_each` entry
3. host `vars`
4. host group `vars`
5. service `vars`
6. check `vars`

## Precedence

host service > host > host group > service > check > defaults, key by key.

- Tables (`vars`, `thresholds`) merge per key.
- Lists are replaced; `groups` add up.
- Two host groups of a host setting `host_check`, `graph` or a var
  differently: **error**. Set it on the host.

```
hosts.toml:8: error: host "h1": host groups set vars.x differently: a (groups.toml:1) and b (groups.toml:5)
```

## One service per item: for_each

One service per entry of a host var. Name: `<service>_<key>`.

```toml
[check.disk]
command = "check_disk -w $warn$ -c $crit$ -p $path$"
vars = { warn = "20%", crit = "10%" }

[service.disk]
for_each = "disks"
interval = "5m"

[hostgroup.linux]
members = ["db*"]
services = ["disk"]
vars.disks.root = { path = "/" }                              # every member

[[host]]
name = "db01"
vars.disks.data = { path = "/srv", warn = "10%", crit = "5%" }
```

```
db01/disk_data  every 5m     check_disk -w '10%' -c '5%' -p /srv
db01/disk_root  every 5m     check_disk -w '20%' -c '10%' -p /
```

Override one: `[host.service.disk_root]`. All: `[host.service.disk]`.

Nested: dotted path. Entry key: `$service.key$`. Entry `{}`: no vars.

```toml
[check.https]
command = "check_http -S -H $host.vars.site.name$.$service.key$"

[service.https]
for_each = "site.tld"

[[host]]
name = "web01"
services = ["https"]
vars.site.name = "klarsocial"
vars.site.tld.eu = {}
vars.site.tld.de = {}
```

```
web01/https_de  every 1m     check_http -S -H klarsocial.de
web01/https_eu  every 1m     check_http -S -H klarsocial.eu
```

## Host state

`host_check` names a service on the host:

| Service | Host |
|---|---|
| OK, WARNING | UP |
| CRITICAL, UNKNOWN | DOWN |

- No host check: always UP.
- Retries (`max_attempts`) count only CRITICAL and UNKNOWN; WARNING ends them.
- Host DOWN (HARD): its other services don't notify.
- The host check notifies as the host: "web01 is DOWN".

## Graphs

The host page graphs `graph` (a service name), default the host check.

```toml
[hostgroup.linux]
graph = "load"
```

No graphs for a check or service:

```toml
[check.backup]
graphs = false
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
db01/disk_data                                              services.toml:1
├─ from
│  ├─ host db01                                             hosts.toml:6
│  │  └─ vars.disks.data: for_each entry                    hosts.toml:6
│  ├─ service disk                                          services.toml:1
│  │  └─ hostgroup linux: listed                            hosts.toml:1
│  └─ check disk                                            checks.toml:1
│     └─ command: check_disk -w $warn$ -c $crit$ -p $path$
└─ values
   ├─ command: check_disk -w '10%' -c '5%' -p /srv          checks.toml:1
   │  └─ check disk                                         checks.toml:1
   ├─ interval: 5m                                          services.toml:1
   │  └─ service disk                                       services.toml:1
   ...
   └─ vars.warn: 10%                                        hosts.toml:6
      └─ host db01, vars.disks.data                         hosts.toml:6
```

Web UI: **Config Tree** on host, service and group pages and every config
form.

```sh
watcher config explain service disk     # hosts that have it, and why
watcher config check --services         # every service with its command
```
