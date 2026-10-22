# Watcher

Infrastructure monitoring: hosts, services, metrics, alerts. Nagios plugins
and any script that prints a result work as checks.

## Requirements

- Rust 1.94 or later, Node.js (LTS) to build the web UI
- PostgreSQL, ideally with TimescaleDB (optional, see below)
- Monitoring plugins (optional), e.g. `apt install monitoring-plugins`

## Build

```sh
npm --prefix web ci
npm --prefix web run build    # before cargo: the UI is built into the binary
cargo build --release         # → target/release/watcher
```

## Database

Optional. Without one: no history, graphs or logins, and the web UI only
listens on 127.0.0.1.

```sh
docker run -d --name watcher-db -p 127.0.0.1:5432:5432 \
  -e POSTGRES_USER=watcher -e POSTGRES_PASSWORD=watcher -e POSTGRES_DB=watcher \
  timescale/timescaledb:latest-pg18
```

URL: `postgres://watcher:watcher@localhost:5432/watcher`. The schema is
created on first start.

## Setup

In an empty directory:

```sh
watcher setup     # browser: open the URL from the log, enter the setup token
watcher init      # or: questions in the terminal
```

Both ask for the database, web address, admin account, email and the alert
contact, and can add checks for this machine. They write:

```
watcher.toml        listen address, database, SMTP
conf.d/checks.toml  what to run
conf.d/roles.toml   which services a kind of host gets
conf.d/hosts.toml   the hosts
conf.d/contacts.toml, conf.d/notify.toml   who gets alerts
```

Start:

```sh
watcher run       # reads ./watcher.toml; web UI on http://127.0.0.1:8080
```

## Adding hosts and services

| Term | Meaning |
|---|---|
| check | what to run: a command and its defaults |
| role | the services a kind of host gets |
| host | something to monitor |
| service | a check on a host |

**Web UI:** Configuration → Hosts → Add Host, pick roles; Configuration →
Roles / Checks likewise. Every save shows what changes before Apply.
**Clone** copies an object; `web[03-10]` as names makes eight copies.

**Files:** any `*.toml` below `conf.d/`. Saved changes are loaded within a
second; with errors, the running config stays and the log says why.

Check before saving, and see where a service's values come from:

```sh
watcher config check --services
watcher config explain web01/http
```

### A host

```toml
[[host]]
name = "web01"
address = "10.0.0.5"          # default: the name
roles = ["linux"]
tags = ["web", "prod"]
```

### A check

Bare plugin names are found in `checks/` next to watcher.toml and in the
monitoring-plugins directories. `$name$` is a variable, `$host.address$`
the host's address.

```toml
[check.ping]
command = "check_ping -H $host.address$ -w $warn$ -c $crit$"
vars = { warn = "100,20%", crit = "500,60%" }
```

### A role

Services listed in a role run on every host with the role. `service.ping =
{}` uses the check of the same name. `host_check` gives hosts UP/DOWN.

```toml
[role.linux]
host_check = "ping"
interval = "1m"
service.ping = {}
service.load = { check = "load" }
```

### Roles by tag

```toml
[role.web]
hosts = { tags = ["web"] }    # every host tagged web
service.http = {}
```

### One service on one host

```toml
[[host]]
name = "db01"
roles = ["linux"]
  [host.service.backup_age]
  command = "/opt/scripts/backup_age.sh"
```

### Different values on one host

Host values win over the role's, the role's over the check's.

```toml
[[host]]
name = "web02"
roles = ["linux"]
vars.warn = "200,30%"         # used by $warn$ in ping
  [host.service.load]
  enabled = false             # this host doesn't get it
```

### One service per disk

```toml
[check.disk]
command = "check_disk -w $warn$ -c $crit$ -p $path$"
vars = { warn = "20%", crit = "10%" }

[role.linux]
service.disk = { for_each = "disks" }

[[host]]
name = "db01"
roles = ["linux"]
vars.disks.root = { path = "/" }                               # → disk_root
vars.disks.data = { path = "/srv", warn = "10%", crit = "5%" } # → disk_data
```

### A script as check

Exit code 0–3 is OK, WARNING, CRITICAL, UNKNOWN. `key=value` output becomes
metrics.

```toml
[check.uptime]
shell = true
command = 'echo "uptime=$(cut -d" " -f1 /proc/uptime)s"'
```

### Thresholds on metrics

```toml
[check.http]
command = "check_http -H $host.address$"
thresholds.time = { warn = "> 500ms", crit = "> 2s" }
```
