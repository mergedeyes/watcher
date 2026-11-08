# Introduction

Watcher monitors infrastructure: hosts, services, metrics and alerts. It is
one binary with a built-in web UI. You configure it with TOML files or in
the web UI, and it stores results in PostgreSQL. Nagios plugins and any
script that prints a result work as checks.

## Concepts

| Term | Meaning |
|---|---|
| [check](objects/check.md) | What to run: a command, its default values, thresholds. |
| [service](objects/service.md) | A check with its settings, put on hosts. On a host, a service has the ID `host/service`. |
| [host](objects/host.md) | A machine or device to monitor. |
| [host group](objects/hostgroup.md) | Hosts that get the same services and settings. |
| [service group](objects/servicegroup.md) | Services that are assigned or viewed together. |
| host check | The service that decides whether a host is UP or DOWN. |
| [contact](objects/contact.md) | Someone who receives notifications by email or webhook. |
| [contact group](objects/contactgroup.md) | A set of contacts. |
| [notify rule](objects/notify.md) | Which contacts get which notifications. |
| [downtime group](objects/downtimegroup.md) | Hosts with recurring downtime windows. |
| [sink](objects/sink.md) | A second database that receives a copy of all results. |

## States

| Services | Hosts |
|---|---|
| OK, WARNING, CRITICAL, UNKNOWN, and PENDING before the first result | UP, DOWN |

A new problem is first SOFT. It becomes HARD once it was seen
`max_attempts` times in a row. Only HARD changes send notifications, so a
single bad result doesn't wake anyone up. See
[SOFT and HARD states](objects/check.md#soft-and-hard-states).

## What works today

Single server:

- **Checks:** Nagios plugins; scripts with Nagios, `key=value` or JSON
  output; passive results over the API; thresholds with units; built-in
  ping.
- **Checks over SSH:** run on the hosts without an agent; keys or passwords
  stored encrypted; pinned host keys; scripts copied to the hosts.
- **State handling:** SOFT/HARD, retries, host UP/DOWN, flap detection,
  freshness of passive results.
- **Configuration:** files and web UI, services, groups, `for_each`, Config
  Tree, validation with file and line, reload on save.
- **Web UI:** Tactical Overview, hosts, services, problems, groups, alert
  history, graphs, configuration forms and file editor.
- **Operations:** acknowledgements, one-off and recurring downtimes, muted
  notifications, disabled checks, re-check, process info, scheduling queue.
- **Notifications:** email, webhooks.
- **Storage:** PostgreSQL or TimescaleDB, SQL views, read-only database
  login, copies to another PostgreSQL (sinks).
- **Access:** users, sessions, API tokens, HTTPS; the full API, everything
  the web UI does.

*Not yet:*

| Feature | Phase |
|---|---|
| Nodes and hub, UNREACHABLE state | 2 |
| More built-in checks (TCP, HTTP, DNS, TLS) | 2 / 5 |
| Data repair in graphs, rollups, retention | 3 |
| IBM i, AIX, HMC, MQ checks | 3b |
| Dependencies, escalations, repeated notifications | 4 |
| Tenants, roles and permissions, TOTP, more notification channels | 4 |
| SNMP, Prometheus, WinRM, discovery, Nagios config import | 5 |
