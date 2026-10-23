# Watcher documentation

State: October 2026, single-server version (Phase 1). Planned features are
marked *not yet*.

## Start

- [Introduction](introduction.md): what Watcher is, goals, status
- [Installation](install.md): build, database, setup, running as a service
- [First steps](guides/first-steps.md): from an empty directory to the first alert

## Guides

- [Checks](guides/checks.md): commands, scripts, output formats, thresholds, passive checks
- [Hosts and services](guides/hosts-and-services.md): assigning services, vars, overrides, `for_each`
- [Groups](guides/groups.md): host groups, service groups, contact groups
- [Notifications](guides/notifications.md): SMTP, contacts, notify rules, webhooks
- [Web UI](guides/web-ui.md): status pages, configuration, Config Tree
- [Data](guides/data.md): database, SQL, Grafana, sinks

## Reference

- [watcher.toml](reference/watcher-toml.md)
- [Object files](reference/objects.md): every kind and field
- [Selectors](reference/selectors.md): host and service selectors, expressions
- [Command line](reference/cli.md)
- [HTTP API](reference/api.md)

## Examples

- [Examples](examples.md): complete configurations
