# Watcher documentation

This documentation describes the single-server version (Phase 1, November
2026). Features that are planned but not available yet are marked
*not yet*.

## Getting started

- [Introduction](introduction.md): what Watcher is, its goals, what works today
- [Installation](install.md): build, database, setup, running as a service
- [First steps](guides/first-steps.md): from an empty directory to the first alert
- [How the configuration works](guides/configuration.md): files, how objects fit together, which value wins

## Objects

One page per object, each with a simple example, every field, and examples
for common cases.

| Object | What it is |
|---|---|
| [check](objects/check.md) | What to run and how to judge the result: commands, built-in checks, macros, output formats, thresholds, passive checks |
| [service](objects/service.md) | A check with its settings, put on hosts; `for_each` |
| [host](objects/host.md) | A machine to monitor; host services, host state, SSH login |
| [hostgroup](objects/hostgroup.md) | Hosts that get the same services and settings |
| [servicegroup](objects/servicegroup.md) | Services that are assigned or viewed together |
| [contact](objects/contact.md) | Someone who receives notifications |
| [contactgroup](objects/contactgroup.md) | A set of contacts |
| [notify](objects/notify.md) | Who gets which notifications; host and service selectors |
| [downtimegroup](objects/downtimegroup.md) | Hosts with recurring downtime windows |
| [sink](objects/sink.md) | A second database that receives a copy of all results |

## Guides

- [Notifications](guides/notifications.md): SMTP, email format, webhooks, notification log
- [Alerting recipes](guides/alerting-recipes.md): ready-made setups for reminders, escalations, teams, customers and maintenance windows
- [Checks over SSH](guides/ssh.md): running checks on the hosts, secrets, host keys
- [Operations](guides/operations.md): acknowledgements, downtimes, muting, turning checks off
- [Web UI](guides/web-ui.md): pages, logins, editing the configuration
- [Data](guides/data.md): database, SQL views, Grafana

## Reference

- [watcher.toml](reference/watcher-toml.md): listen address, database, TLS, SMTP
- [Command line](reference/cli.md)
- [HTTP API](reference/api.md)

## Examples

- [Examples](examples.md): complete configurations for common setups
