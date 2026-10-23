# First steps

From an empty directory to a monitored host with email alerts. Needs
[monitoring-plugins](https://www.monitoring-plugins.org) and a database
([Installation](../install.md)).

## 1. Setup

```sh
mkdir ~/watcher && cd ~/watcher
watcher init
watcher run
```

Open http://127.0.0.1:8080, log in with the admin account.

## 2. A check

`conf.d/checks.toml`:

```toml
[check.ping]
command = "check_ping -H $host.address$ -w $warn$ -c $crit$"
vars = { warn = "100,20%", crit = "500,60%" }

[check.http]
command = "check_http -H $host.address$ -u $url$"
vars.url = "/"
thresholds.time = { warn = "> 500ms", crit = "> 2s" }
```

Try it first:

```sh
watcher run-check -- check_ping -H 10.0.0.5 -w 100,20% -c 500,60%
```

## 3. A role

`conf.d/roles.toml`:

```toml
[role.web]
host_check = "ping"       # gives the host UP/DOWN
service.ping = {}
service.http = {}
```

## 4. A host

`conf.d/hosts.toml`:

```toml
[[host]]
name = "web01"
address = "10.0.0.5"
roles = ["web"]
```

Saved files load within a second. Log:

```
reloading (file change)
```

## 5. Check the result

```sh
watcher config check --services
```

```
OK: 3 files, 1 host, 1 role, 2 checks, 2 services

  web01/http  every 1m     check_http -H 10.0.0.5 -u /
  web01/ping  every 1m     check_ping -H 10.0.0.5 -w '100,20%' -c '500,60%'
```

Web UI: Hosts → web01. Config Tree shows where each value comes from.

## 6. Alerts

`conf.d/contacts.toml`:

```toml
[contact.oncall]
email = "oncall@example.com"

[[notify]]
contacts = ["oncall"]
hosts = ["web*"]
```

SMTP in `watcher.toml` ([reference](../reference/watcher-toml.md#smtp)),
then:

```sh
watcher notify test oncall
```

## 7. Same in the web UI

Configuration → Checks / Roles / Hosts → Add. Every save shows the
changes (+ ~ −) before Apply. The UI writes to the same files.

## Next

- [Checks](checks.md): own scripts, output formats
- [Hosts and roles](hosts-and-roles.md): roles by tag, per-host values,
  one service per disk
- [Notifications](notifications.md)
