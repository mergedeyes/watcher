# Watcher documentation

This documentation describes Watcher as of November 2026: the
single-server version, and the first part of nodes, which can join a hub
and stay connected. Features that are planned but not available yet are
marked *not yet*.

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
| [dependency](objects/dependency.md) | Quiet dependents while their master is down, for example hosts behind a router |
| [downtimegroup](objects/downtimegroup.md) | Hosts with recurring downtime windows |
| [timeperiod](objects/timeperiod.md) | When notify rules apply, for example business hours |
| [sink](objects/sink.md) | A second database that receives a copy of all results |

## Guides

- [Notifications](guides/notifications.md): SMTP, email format, webhooks, Slack, Teams, Telegram, ntfy, Gotify, PagerDuty, Jira, scripts, notification log
- [Alerting recipes](guides/alerting-recipes.md): ready-made setups for reminders, escalations, teams, customers and maintenance windows
- [Checks over SSH](guides/ssh.md): running checks on the hosts, secrets, host keys
- [Monitoring Proxmox VE](guides/proxmox.md): cluster, nodes, storages, VMs and containers, backups and updates through the API
- [Nodes](guides/nodes.md): a hub and Watchers at other sites that connect to it
- [Operations](guides/operations.md): acknowledgements, downtimes, muting, turning checks off
- [Web UI](guides/web-ui.md): pages, logins, two-factor authentication, editing the configuration
- [Data](guides/data.md): database, SQL views, Grafana

## Reference

- [watcher.toml](reference/watcher-toml.md): listen address, database, hub and node, TLS, SMTP
- [Command line](reference/cli.md)
- [HTTP API](reference/api.md)

## Examples

- [Examples](examples.md): complete configurations for common setups
