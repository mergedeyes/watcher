# Examples

Complete `conf.d/` setups. Each passes `watcher config check`. Plugins:
monitoring-plugins.

More in the repository: [examples/local](../examples/local),
[examples/demo](../examples/demo) (no plugins needed),
[examples/sinks.toml](../examples/sinks.toml).

## One Linux server

The Watcher machine itself.

```toml
# conf.d/checks.toml
[check.disk]
command = "check_disk -w $warn$ -c $crit$ -p $path$"
vars = { warn = "20%", crit = "10%" }

[check.load]
command = "check_load -r -w 2,1.5,1 -c 4,3,2"

[check.swap]
command = "check_swap -w 50% -c 20%"

[check.procs]
command = "check_procs -w 400 -c 600"
```

```toml
# conf.d/services.toml
[service.disk]
for_each = "disks"

[service.load]

[service.swap]

[service.procs]
interval = "5m"
```

```toml
# conf.d/hosts.toml
[[host]]
name = "localhost"
services = ["disk", "load", "swap", "procs"]
graph = "load"
vars.disks.root = { path = "/" }
vars.disks.var = { path = "/var" }
```

```toml
# conf.d/contacts.toml
[contact.admin]
email = "admin@example.com"

[[notify]]
contacts = ["admin"]
```

## Web and database servers

Host groups by name, UP/DOWN by ping, a service group for every server, a
"Webshop" view, DBAs only for database problems.

```toml
# conf.d/checks.toml
[check.ping]
command = "check_ping -H $host.address$ -w $warn$ -c $crit$"
vars = { warn = "100,20%", crit = "500,60%" }

[check.ssh]
command = "check_ssh -H $host.address$"

[check.http]
command = "check_http -H $host.address$ -u $url$"
vars.url = "/"
thresholds.time = { warn = "> 500ms", crit = "> 2s" }

[check.https_cert]
command = "check_http -H $host.address$ -S -C 21,7"

[check.pgsql]
command = "check_pgsql -H $host.address$ -d $db$"
vars.db = "postgres"
```

```toml
# conf.d/services.toml
[service.ping]
[service.ssh]
[service.http]
[service.https_cert]
[service.pgsql]
```

```toml
# conf.d/groups.toml
[servicegroup.server]
title = "Server Basics"
services = ["ping", "ssh"]

[servicegroup.shop]
title = "Webshop"
services = ["http", "pgsql"]

[hostgroup.all]
members = ["*"]
servicegroups = ["server"]
host_check = "ping"

[hostgroup.web-servers]
title = "Web Servers"
members = ["web*", "test01"]
services = ["http", "https_cert"]

[hostgroup.databases]
title = "Databases"
members = ["db*"]
services = ["pgsql"]

[hostgroup.prod]
members = ["web01", "web02", "db01"]
```

```toml
# conf.d/hosts.toml
[[host]]
name = "web01"
address = "10.0.1.11"

[[host]]
name = "web02"
address = "10.0.1.12"
vars.url = "/health"

[[host]]
name = "db01"
address = "10.0.2.21"
vars.db = "shop"

[[host]]
name = "test01"
address = "10.0.9.1"
  [host.service.https_cert]
  enabled = false                             # self-signed on test
```

```toml
# conf.d/contacts.toml
[contact.oncall]
email = "oncall@example.com"

[contact.alice]
email = "alice@example.com"
groups = ["dba"]

[contact.bob]
email = "bob@example.com"
groups = ["dba"]

[contactgroup.dba]
title = "DBAs"
```

```toml
# conf.d/notify.toml
[[notify]]
name = "prod"
contacts = ["oncall"]
hosts = { groups = ["prod"] }

[[notify]]
name = "dba"
contact_groups = ["dba"]
services = { checks = ["pgsql"] }
states = ["critical"]
```

## Many websites

One HTTP service per site with `for_each`, all on one host entry.

```toml
# conf.d/checks.toml
[check.http]
command = "check_http -H $vhost$ -u $url$ -S"
vars.url = "/"
thresholds.time = { warn = "> 1s", crit = "> 5s" }

[check.cert]
command = "check_http -H $vhost$ -S -C 21,7"
interval = "1h"
```

```toml
# conf.d/services.toml
[service.http]
for_each = "sites"
groups = ["websites"]

[service.cert]
for_each = "sites"
```

```toml
# conf.d/hosts.toml
[[host]]
name = "websites"
services = ["http", "cert"]
vars.sites.shop = { vhost = "shop.example.com" }
vars.sites.blog = { vhost = "blog.example.com", url = "/feed" }
vars.sites.api = { vhost = "api.example.com", url = "/v1/health" }
```

```toml
# conf.d/groups.toml
[servicegroup.websites]
title = "Websites"
```

Services: `websites/http_shop`, `websites/http_blog`, `websites/http_api`,
`websites/cert_shop`, ...

## Customers (MSP)

A host group per customer, alerts to the customer and to the NOC.

```toml
# conf.d/checks.toml
[check.ping]
command = "check_ping -H $host.address$ -w 200,20% -c 800,60%"

[check.rdp]
command = "check_tcp -H $host.address$ -p 3389"

[check.smtp]
command = "check_smtp -H $host.address$"
```

```toml
# conf.d/services.toml
[service.ping]
[service.rdp]
[service.smtp]
```

```toml
# conf.d/groups.toml
[hostgroup.all]
members = ["*"]
services = ["ping"]
host_check = "ping"

[hostgroup.windows]
services = ["rdp"]

[hostgroup.mail]
members = ["*mail*"]
services = ["smtp"]

[hostgroup.acme]
title = "ACME Corp"
members = ["acme-*"]

[hostgroup.globex]
title = "Globex"
members = ["globex-*"]
```

```toml
# conf.d/hosts/acme.toml
[[host]]
name = "acme-dc01"
address = "192.0.2.10"
groups = ["windows"]

[[host]]
name = "acme-mail"
address = "192.0.2.25"
```

```toml
# conf.d/hosts/globex.toml
[[host]]
name = "globex-fs01"
address = "198.51.100.5"
groups = ["windows"]
```

```toml
# conf.d/notify.toml
[contact.noc]
email = "noc@msp.example"
webhook = "https://chat.msp.example/hooks/watcher"

[contact.acme-it]
email = "it@acme.example"

[[notify]]
name = "noc"
contacts = ["noc"]

[[notify]]
name = "acme"
contacts = ["acme-it"]
hosts = { groups = ["acme"] }
states = ["critical", "down"]
on = ["problem", "recovery"]
```

## Backup jobs (passive)

Jobs push their result after each run; no result in 26 h is UNKNOWN.

```toml
# conf.d/services.toml
[service.backup]
passive = true
freshness = "26h"
```

```toml
# conf.d/hosts.toml
[hostgroup.backup]
services = ["backup"]

[[host]]
name = "fileserver"
groups = ["backup"]

[[host]]
name = "mailserver"
groups = ["backup"]
  [host.service.backup]
  freshness = "8h"                            # runs every 6 h
  thresholds.duration = { warn = "> 2h" }
```

```toml
# conf.d/contacts.toml
[contact.backup-admin]
email = "backup@example.com"

[[notify]]
contacts = ["backup-admin"]
services = { name = "backup" }
```

At the end of the backup script, with a token from
`watcher token create backup "backup jobs"`:

```sh
curl -fsS -X POST "https://watcher.example.com/api/v1/services/$(hostname)/backup/result" \
  -H "Authorization: Bearer $(cat /etc/watcher-token)" -H 'content-type: application/json' \
  -d "{\"exit_code\": $rc, \"output\": \"BACKUP rc=$rc | duration=${secs}s size=${bytes}B\"}"
```

## Own scripts

`checks/` next to `watcher.toml`, executable.

`checks/app_queue`, key=value:

```sh
#!/bin/sh
echo "depth=$(redis-cli -h "$1" llen jobs) workers=$(redis-cli -h "$1" scard workers)"
```

`checks/app_health`, JSON:

```sh
#!/bin/sh
curl -fsS "http://$1:8080/health" | jq '{state: (if .ok then "ok" else "critical" end), message: .status, metrics: {latency: .latency_ms}}'
```

```toml
# conf.d/checks.toml
[check.app_queue]
command = "app_queue $host.address$"
thresholds.depth = { warn = "> 1000", crit = "> 10_000" }
thresholds.workers = { crit = "< 1" }

[check.app_health]
command = "app_health $host.address$"
format = "json"
timeout = "10s"
graphs = false
```

```toml
# conf.d/services.toml
[service.queue]
check = "app_queue"

[service.health]
check = "app_health"
interval = "30s"

[servicegroup.app]
title = "App"
services = ["queue", "health"]
```

```toml
# conf.d/hosts.toml
[[host]]
name = "app01"
address = "10.0.5.1"
servicegroups = ["app"]
```

## Copy to a central database

Every result also goes to a reporting PostgreSQL; buffered on disk while
it is down.

```toml
# conf.d/sinks.toml
[[sink]]
name = "reporting"
type = "postgres"
url = "postgres://watcher@reporting.example.com/watcher"
password_file = "reporting.pass"
flush = { mode = "interval", interval = "5m" }
buffer = { max_size = "4GB" }
```

Read-only access for Grafana on the local database:

```sh
watcher db reader grafana
```
