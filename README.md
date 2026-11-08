# Watcher

**Infrastructure monitoring that is as simple to configure as Nagios, and
tells you where every value comes from.**

Watcher checks your hosts and services, stores their metrics, and alerts
you when something breaks. It is a single binary with a built-in web UI.
Your existing Nagios plugins work unchanged, and so does any script that
prints a result.

```toml
[check.http]
command = "check_http -H $host.address$ -u $url$"
vars.url = "/"
thresholds.time = { warn = "> 500ms", crit = "> 2s" }

[service.http]

[hostgroup.web-servers]
members = ["web*"]
services = ["http"]
```

Every host whose name starts with `web` now gets an HTTP check. Hosts with
an address are also pinged, to tell whether they are UP or DOWN.

## Why Watcher

| Elsewhere | Watcher |
|---|---|
| "Where does this value come from?" | The **Config Tree** shows every value with its file, line and layer, in the web UI and with `watcher config explain`. |
| Inheritance surprises: the first template wins, lists get replaced | **One precedence rule**: host > host group > service > check. Two host groups that disagree are an error, not a silent pick. |
| One edit per host, per disk, per URL | Services assigned to **host groups** with name patterns; **`for_each`** creates one service per disk, website or queue. |
| Web UI and config files drift apart | The web UI **edits the same TOML files**, keeps your comments, and shows a diff before applying. File edits load within a second. |
| Notification routing depends on rule order | **Notify rules** without order: every matching rule notifies, each contact gets one message. |
| Data locked in a custom store | **PostgreSQL** (TimescaleDB optional) with plain SQL views for Grafana, BI tools and your own queries. |
| Hours of setup | One binary. A **setup wizard** in the browser or the terminal. |

## Features

- **Checks:** Nagios plugins, your own scripts with Nagios, `key=value` or
  JSON output, passive results pushed over the API, thresholds with units,
  built-in ping.
- **Agentless checks over SSH**, with encrypted secrets, pinned host keys
  and scripts copied to the hosts automatically.
- **Alerting:** SOFT/HARD states, retries, host UP/DOWN, flap detection;
  email and webhooks.
- **Operations:** acknowledgements, one-off and recurring downtimes,
  muting, re-checks.
- **Web UI:** tactical overview, problems, groups, alert history, graphs,
  configuration forms and a file editor with live validation.
- **Data:** PostgreSQL/TimescaleDB, SQL views, read-only logins, copies to
  a central database.
- **API:** everything the web UI does, with tokens.

Coming next: distributed nodes reporting to a central hub, more built-in
checks, data repair in graphs, and native IBM i, AIX, HMC and MQ checks.
See [what works today](docs/introduction.md#what-works-today).

## Quick start

```sh
npm --prefix web ci && npm --prefix web run build
cargo build --release
mkdir ~/watcher && cd ~/watcher
/path/to/target/release/watcher setup    # open the URL from the log
```

Watcher runs without a database, but for history, graphs and logins you
want PostgreSQL; see [Installation](docs/install.md). Then follow
[First steps](docs/guides/first-steps.md) to monitor your first host.

## Documentation

- [Introduction](docs/introduction.md)
- [Installation](docs/install.md)
- [First steps](docs/guides/first-steps.md)
- [How the configuration works](docs/guides/configuration.md)
- [All documentation](docs/README.md)

## License

[AGPL-3.0-only](LICENSE)
