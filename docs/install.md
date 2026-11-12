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
the instance name and time zone, the database, web access (listen
address, public URL, TLS: none, self-signed, certificate files or
Let's Encrypt), the admin account, notifications and checks for this
machine, and shows a summary. **Write & Start** writes the files and
starts Watcher.

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

Or let Watcher get certificates from Let's Encrypt, see
[below](#certificates-from-lets-encrypt).

Or run it behind a reverse proxy and set an `https://` `public_url`. Session
cookies are then marked Secure. Live updates use Server-Sent Events on
`/api/v1/stream`, so turn off response buffering for it, for example with
nginx `proxy_buffering off;`.

Plain HTTP on an address other than loopback logs a warning.

### Certificates from Let's Encrypt

Watcher can get its certificate from Let's Encrypt, or from any other
certificate authority (CA) that speaks ACME, the protocol Let's Encrypt
uses. It renews the certificate by itself and uses the new one without a
restart.

To check that the name is yours, the CA connects to **port 443** of the
name and Watcher answers there (this method is called TLS-ALPN-01). So
the name must point to this server, and port 443 must reach Watcher from
the CA. For Let's Encrypt, that means from the internet.

1. Point the DNS name, for example `watcher.example.com`, to the server.
2. Let Watcher listen on port 443, or forward port 443 to its listen
   port. Port 443 needs a permission that the `watcher` user doesn't
   have; add this line to the `[Service]` section of the
   [systemd unit](#systemd):

   ```ini
   AmbientCapabilities=CAP_NET_BIND_SERVICE
   ```

3. Set the address, the public URL and `[tls.acme]` in `watcher.toml`:

   ```toml
   listen = "0.0.0.0:443"
   public_url = "https://watcher.example.com"

   [tls.acme]
   email = "ops@example.com"     # Let's Encrypt writes here about problems
   ```

   The certificate is for the host of `public_url`. For other or more
   names, set `domains = ["watcher.example.com", "monitor.example.com"]`.
4. Restart Watcher. The log shows
   `ACME: new certificate for watcher.example.com, valid until ...`
   after a few seconds. Until then, Watcher uses a temporary self-signed
   certificate, and browsers warn.

If it doesn't work, the log says why, for example that the CA couldn't
connect. Watcher tries again after 10 minutes, then after longer and
longer pauses (up to 6 hours), because Let's Encrypt allows only a few
failed attempts per hour. To try things out without that limit, use
Let's Encrypt's test CA (its certificates are not trusted by browsers):

```toml
[tls.acme]
directory = "https://acme-staging-v02.api.letsencrypt.org/directory"
```

By setting up `[tls.acme]` you agree to the CA's terms (for Let's Encrypt,
its Subscriber Agreement). The account, the certificate and its key are
kept in `data_dir/acme/`.

**A server that the internet can't reach** can't get a certificate from
Let's Encrypt. Use an internal CA that speaks ACME instead, such as
[step-ca](https://smallstep.com/docs/step-ca/). Set its `directory`, and
`ca` if the system doesn't trust the CA's own certificate yet:

```toml
[tls.acme]
directory = "https://ca.internal:9000/acme/acme/directory"
ca = "/etc/watcher/internal-root.pem"
```

CAs such as ZeroSSL need an *external account binding*: a key ID and a
key from your account at the CA. Set them as `eab_kid` and
`eab_hmac_key`. All options: [`[tls.acme]`](reference/watcher-toml.md#tlsacme).

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
