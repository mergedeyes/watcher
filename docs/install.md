# Installation

## Requirements

| | |
|---|---|
| Build | Rust 1.94+, Node.js LTS |
| Database | PostgreSQL, ideally TimescaleDB. Optional |
| Plugins | Optional, e.g. `apt install monitoring-plugins` |
| OS | Linux |

## Build

```sh
npm --prefix web ci
npm --prefix web run build    # first: the UI is embedded into the binary
cargo build --release
install -m 755 target/release/watcher /usr/local/bin/
```

## Database

Without a database: no history, graphs, alert history or logins; the web
UI listens on 127.0.0.1 only.

TimescaleDB in Docker:

```sh
docker run -d --name watcher-db --restart unless-stopped \
  -p 127.0.0.1:5432:5432 -v watcher-db:/var/lib/postgresql/data \
  -e POSTGRES_USER=watcher -e POSTGRES_PASSWORD=watcher -e POSTGRES_DB=watcher \
  timescale/timescaledb:latest-pg18
```

Existing PostgreSQL:

```sql
CREATE ROLE watcher LOGIN PASSWORD 'secret';
CREATE DATABASE watcher OWNER watcher;
```

URL: `postgres://watcher:secret@localhost/watcher`. Tables are created on
start. With the `timescaledb` extension available, samples become a
hypertable with compression.

## Setup

In an empty directory (e.g. `/etc/watcher`). Both ways write the same files.

**Browser:**

```sh
watcher setup                       # http://127.0.0.1:8080
watcher setup --listen 0.0.0.0:8080 # remote machine
```

The log prints a setup token; enter it on the page. Steps: instance,
database, web access (listen, public URL, TLS), admin, notifications, this
machine, summary. **Write & Start** writes the files and starts the server.

**Terminal:**

```sh
watcher init /etc/watcher
```

**Headless:**

```sh
watcher init /etc/watcher --answers answers.toml
```

Answers file: [examples/init-answers.toml](../examples/init-answers.toml).

Result:

```
watcher.toml          listen, database, SMTP, TLS (0600)
smtp.pass             SMTP password (0600), if given
conf.d/contacts.toml  contact "admins"
conf.d/notify.toml    notify rule for "admins"
conf.d/checks.toml    checks for this machine, if chosen
conf.d/roles.toml     role for this machine
conf.d/hosts.toml     host localhost
```

Existing files are kept unless `--force`.

## Manual setup

Minimal `watcher.toml`:

```toml
listen = "0.0.0.0:8080"
public_url = "https://watcher.example.com"

[database]
url = "postgres://watcher:secret@localhost/watcher"
```

Admin account:

```sh
watcher user add admin --admin -c /etc/watcher/watcher.toml
```

All options: [watcher.toml](reference/watcher-toml.md).

## Run

```sh
watcher run                              # ./watcher.toml, else /etc/watcher/watcher.toml
watcher run /etc/watcher/watcher.toml
watcher run --all                        # log every result, not only changes
```

`watcher run` with a path that doesn't exist yet starts the browser setup.

Reload: object files are watched; `SIGHUP` reloads too. Changes to
`watcher.toml` need a restart.

## systemd

`/etc/systemd/system/watcher.service`:

```ini
[Unit]
Description=Watcher
After=network-online.target postgresql.service
Wants=network-online.target

[Service]
User=watcher
ExecStart=/usr/local/bin/watcher run /etc/watcher/watcher.toml
ExecReload=/bin/kill -HUP $MAINPID
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

```sh
useradd --system --home /etc/watcher watcher
chown -R watcher: /etc/watcher
systemctl enable --now watcher
```

The `watcher` user runs every check: give it the permissions the plugins
need (e.g. `check_icmp` needs `CAP_NET_RAW` or setuid).

## HTTPS

Either in `watcher.toml`:

```toml
[tls]
cert = "/etc/ssl/watcher.pem"
key = "/etc/ssl/watcher.key"
```

```toml
[tls]
self_signed = true      # generated in data/tls/
```

or a reverse proxy with an `https://` `public_url`. Session cookies are
then marked Secure. Live updates use Server-Sent Events
(`/api/v1/stream`): turn off response buffering for it, e.g. nginx
`proxy_buffering off;`.

Plain HTTP on a non-loopback address logs a warning.

## Upgrades

Replace the binary, restart. Migrations run on start. Rerun
`watcher db reader <name>` for read-only DB logins.

## Development

```sh
docker compose up -d --wait     # TimescaleDB :55432, PostgreSQL :55433, Mailpit :8025
watcher run examples/demo/watcher.toml \
  --database postgres://watcher:watcher@localhost:55432/postgres
npm --prefix web run dev        # UI with hot reload, proxies /api to :8080
```

[examples/demo](../examples/demo) shows every state without plugins.
