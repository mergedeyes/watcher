# Installation

## Requirements

| | |
|---|---|
| Build | Rust 1.94 or later, Node.js LTS |
| Database | PostgreSQL, ideally with TimescaleDB. Optional, but needed for history, graphs and logins. |
| Plugins | Optional. Ping, TCP, HTTP, DNS and TLS checks are built in; for more, for example `apt install monitoring-plugins` |
| OS | Linux |

## Build

The web UI is built first, because it is embedded into the binary.

```sh
npm --prefix web ci
npm --prefix web run build
cargo build --release
install -m 755 target/release/watcher /usr/local/bin/
```

## Database

Watcher runs without a database, but then it has no history, no graphs, no
alert history and no logins, and the web UI only listens on 127.0.0.1.

TimescaleDB in Docker:

```sh
docker run -d --name watcher-db --restart unless-stopped \
  -p 127.0.0.1:5432:5432 -v watcher-db:/var/lib/postgresql/data \
  -e POSTGRES_USER=watcher -e POSTGRES_PASSWORD=watcher -e POSTGRES_DB=watcher \
  timescale/timescaledb:latest-pg18
```

Or an existing PostgreSQL server:

```sql
CREATE ROLE watcher LOGIN PASSWORD 'secret';
CREATE DATABASE watcher OWNER watcher;
```

The URL is then `postgres://watcher:secret@localhost/watcher`. Watcher
creates its tables on start. If the `timescaledb` extension is available,
metric samples are stored in a compressed hypertable.

## Setup

Run the setup in an empty directory, for example `/etc/watcher`. There are
three ways, and all of them write the same files.

**In the browser:**

```sh
watcher setup                       # http://127.0.0.1:8080
watcher setup --listen 0.0.0.0:8080 # to set up a remote machine
```

The log prints a setup token; enter it on the page. The wizard asks for
the instance name, the database, web access (listen address, public URL,
TLS), the admin account, notifications and checks for this machine, and
shows a summary. **Write & Start** writes the files and starts Watcher.

For the admin account you can turn on
[two-factor authentication](guides/web-ui.md#two-factor-authentication)
right away: tick **Two-Factor**, scan the QR code with an authenticator
app, and confirm with a code from the app. After **Write & Start**, the
page shows the ten recovery codes once. Save them before you continue to
the login.

**In the terminal:**

```sh
watcher init /etc/watcher
```

**Without questions**, from an answers file
([examples/init-answers.toml](../examples/init-answers.toml)):

```sh
watcher init /etc/watcher --answers answers.toml
```

The result:

```
watcher.toml          listen address, database, SMTP, TLS (mode 0600)
smtp.pass             SMTP password (mode 0600), if given
conf.d/contacts.toml  the contact "admins"
conf.d/notify.toml    a notify rule for "admins"
conf.d/checks.toml    checks for this machine, if chosen
conf.d/services.toml  services and a service group
conf.d/hosts.toml     the host localhost
```

Existing files are kept unless you add `--force`.

## Manual setup

A minimal `watcher.toml`:

```toml
listen = "0.0.0.0:8080"
public_url = "https://watcher.example.com"

[database]
url = "postgres://watcher:secret@localhost/watcher"
```

Then create an admin account:

```sh
watcher user add admin --admin -c /etc/watcher/watcher.toml
```

After the first login, turn on
[two-factor authentication](guides/web-ui.md#two-factor-authentication)
for the account on the Account page.

All options: [watcher.toml](reference/watcher-toml.md).

## Run

```sh
watcher run                              # ./watcher.toml, else /etc/watcher/watcher.toml
watcher run /etc/watcher/watcher.toml
watcher run --all                        # log every result, not only changes
```

If the given `watcher.toml` doesn't exist yet, `watcher run` starts the
setup in the browser.

Object files are watched and reload on save; `SIGHUP` reloads them too.
Changes to `watcher.toml` need a restart.

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

All checks run as the `watcher` user. Give it the permissions your plugins
need; for example, `check_icmp` needs `CAP_NET_RAW` or setuid.

## HTTPS

Either let Watcher handle TLS in `watcher.toml`:

```toml
[tls]
cert = "/etc/ssl/watcher.pem"
key = "/etc/ssl/watcher.key"
```

```toml
[tls]
self_signed = true      # certificate generated in data/tls/
```

Or run it behind a reverse proxy and set an `https://` `public_url`. Session
cookies are then marked Secure. Live updates use Server-Sent Events on
`/api/v1/stream`, so turn off response buffering for it, for example with
nginx `proxy_buffering off;`.

Plain HTTP on an address other than loopback logs a warning.

## Upgrades

Replace the binary and restart. Database migrations run on start. Run
`watcher db reader <name>` again for each read-only database login.

## Development

```sh
docker compose up -d --wait     # TimescaleDB :55432, PostgreSQL :55433, Mailpit :8025
watcher run examples/demo/watcher.toml \
  --database postgres://watcher:watcher@localhost:55432/postgres
npm --prefix web run dev        # web UI with hot reload, forwards /api to :8080
```

[examples/demo](../examples/demo) shows every state without any plugins.
