# Introduction

Watcher is infrastructure monitoring: hosts, services, metrics, alerts.
One binary, configured with TOML files or the web UI, results in
PostgreSQL. Nagios plugins and any script that prints a result work as
checks.

## Goals

- **Monitoring for service providers and in-house ops.** Hosts, services,
  metrics, alerting, reporting.
- **Config as simple as Nagios, less verbose.** Checks, services, hosts and
  groups; services assigned to groups instead of host × service lists.
  Every value traceable to its file and line.
- **Any script is a check.** Its output is the data. Nagios plugins work
  unchanged.
- **Data easy to reach and repair.** Plain SQL views; range edits in the
  graphs (delete, interpolate).
- **Distributed.** Small nodes at customer sites report to a central hub
  over one outbound connection.
- **Efficient.** Nodes on a Raspberry Pi or a 512 MB VM; one hub for
  10,000 hosts.
- **Native integrations** where a script isn't enough, e.g. IBM i, AIX,
  Power HMC, IBM MQ.
- **Open source:** AGPL-3.0-only.

## Why Watcher

| Problem elsewhere | Watcher |
|---|---|
| "Where does this value come from?" | Config Tree on every object: each value with its file, line and layer (`watcher config explain`, web UI) |
| Inheritance surprises (first template wins, lists replaced) | One precedence rule: host > host group > service > check > defaults. Two host groups setting the same thing differently is an error, not a silent pick |
| One edit per host, disk, URL | Services and service groups assigned to host groups, members by name pattern; `for_each` makes one service per disk, site, queue |
| UI and files out of sync | The UI edits the same TOML files, keeps comments, shows a diff before Apply. File edits reload within a second |
| Notification routing by order | `[[notify]]` rules: every matching rule notifies, no order |
| Data locked in a custom store | PostgreSQL (TimescaleDB optional). Views for SQL, Grafana, BI |
| Heavy setup | One binary with the web UI built in. Setup wizard in the browser or terminal |

## Concepts

| Term | Meaning |
|---|---|
| check | What to run: command, defaults, thresholds |
| service | A check with its settings, assigned to hosts; on a host: id `host/service` |
| host | Something to monitor |
| host group | Hosts that get the same services and settings |
| service group | Services assigned together |
| host check | The service that gives a host UP/DOWN |
| contact | Who gets alerts: email, webhook |
| notify rule | Which contacts get which alerts |
| contact group | Several contacts |
| sink | Where results are copied to, with a disk buffer |

States: OK, WARNING, CRITICAL, UNKNOWN, PENDING (no result yet); hosts UP,
DOWN. SOFT until confirmed `max_attempts` times, then HARD. Only HARD
changes notify.

## Status

Works today (single server):

- Checks: Nagios plugins, scripts (Nagios, `key=value` or JSON output),
  passive results over the API, thresholds with units
- State engine: SOFT/HARD, retries, host UP/DOWN, flap detection,
  freshness
- Config: files and web UI, services, groups, `for_each`, Config Tree,
  validation with file and line, hot reload
- Web UI: Tactical Overview, hosts, services, problems, groups, alert
  history, graphs, configuration forms and file editor
- Operations: acknowledgements, downtimes (one-off and recurring
  downtime groups), muted notifications, disabled checks, re-check,
  process info, scheduling queue
- Notifications: email, webhooks
- Storage: PostgreSQL/TimescaleDB, SQL views, read-only DB role, sinks to
  remote PostgreSQL
- Users, sessions, API tokens, HTTPS; the full API (everything the UI
  does) with tokens

*Not yet:*

| Feature | Phase |
|---|---|
| Nodes and hub, UNREACHABLE | 2 |
| Built-in checks (ICMP, TCP, HTTP, DNS, TLS), checks over SSH | 2 / 5 |
| Data repair in graphs, rollups, retention | 3 |
| IBM i, AIX, HMC, MQ checks | 3b |
| Dependencies, escalations, re-notification | 4 |
| Tenants, roles/permissions, TOTP, more notification channels | 4 |
| SNMP, Prometheus, WinRM, discovery, Nagios config import | 5 |

Until checks over SSH exist, checks run on the Watcher machine: remote
hosts get what works from outside (ping, `check_http`, ...), or
`check_by_ssh` as a normal plugin.

The UI loads all services at once; fine for hundreds of hosts, not yet for
thousands.
