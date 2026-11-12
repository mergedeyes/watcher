# watcher.toml

`watcher.toml` holds what Watcher needs before it can read the object files
and the database: where to listen, where the files are, the database, TLS
and SMTP. Changes need a restart.

Watcher looks for it at the path you give on the command line, then at
`./watcher.toml`, then at `/etc/watcher/watcher.toml`. Relative paths
inside the file are relative to the file. Unknown keys are errors, so typos
don't go unnoticed.

```toml
instance_name = "msp-hub"
timezone = "Europe/Berlin"
listen = "0.0.0.0:8443"
public_url = "https://watcher.example.com:8443"
conf_dir = "conf.d"
data_dir = "data"
config_mode = "hybrid"
check_dirs = ["checks", "/usr/lib/nagios/plugins"]

[database]
url = "postgres://watcher:secret@localhost/watcher"

[tls]
cert = "/etc/ssl/watcher.pem"
key = "/etc/ssl/watcher.key"

[smtp]
host = "mail.example.com"
port = 587
tls = "starttls"
username = "watcher"
password_file = "smtp.pass"
from = "Watcher <watcher@example.com>"
```

## Top level

| Key | Default | Description |
|---|---|---|
| `role` | `standalone` | `standalone`: one server. `hub`: also lets [nodes](../guides/nodes.md) at other sites connect, see [`[hub]`](#hub); needs `[database]`. `node`: only keeps the connection to its hub, see [`[node]`](#node); `watcher node join` sets it. |
| `instance_name` | | Name of this installation, shown in the sidebar, the page title and the mail subject. |
| `timezone` | the system's | IANA time zone name, like `Europe/Berlin`. Used for downtime group windows and time periods. `watcher init` and the setup in the browser write it, with this server's zone as the suggestion. |
| `listen` | `127.0.0.1:8080` | Address and port of the web UI and API. Any address other than loopback needs a database, because logins need one. |
| `public_url` | | The URL users open Watcher with. Used for links in notifications. With `https://`, session cookies are marked Secure. |
| `conf_dir` | `conf.d` | Directory with the object files. Every `*.toml` below it is read. |
| `data_dir` | `data` | Directory for sink buffers, the instance ID, the self-signed certificate, the ACME account and certificate (`acme/`), a hub's certificate authority (`ca/`), a node's certificates (`node/`) and `secret.key`, which encrypts [SSH secrets](../guides/ssh.md#secrets) and the secrets of [two-factor authentication](../guides/web-ui.md#two-factor-authentication). |
| `config_mode` | `hybrid` | `hybrid`: web UI and files can both change the config. `files`: the web UI is read-only. `ui`: files are not watched. See [Config mode](../guides/web-ui.md#config-mode). |
| `check_dirs` | `checks`, then the plugin directories | Where commands without a path are looked up, before `PATH`. |

The default plugin directories, as far as they exist:
`/usr/lib/monitoring-plugins`, `/usr/lib/nagios/plugins`,
`/usr/lib64/nagios/plugins`, `/usr/local/libexec/monitoring-plugins`,
`/usr/local/nagios/libexec`.

## [database]

| Key | Default | Description |
|---|---|---|
| `url` | | `postgres://user:password@host[:port]/db` |

`--database` on the command line and the environment variable
`WATCHER_DATABASE_URL` win over this value.

Without `[database]`, Watcher runs without a database: current states only,
in memory, no history and no logins. See [Data](../guides/data.md).

## [hub]

With `role = "hub"`: where [nodes](../guides/nodes.md) connect. The whole
section is optional.

| Key | Default | Description |
|---|---|---|
| `listen` | `0.0.0.0:7443` | Address and port nodes connect to. Nodes log in with their certificate (mutual TLS); only joining works without one, with a one-time token. |
| `url` | the host of `public_url`, else the `listen` address unless it is `0.0.0.0`, else this machine's name; with the port of `listen` | How nodes reach the hub, for example `https://hub.example.com:7443`. It goes into join commands. |

The hub keeps its certificate authority in `data_dir/ca/`. Keep
`ca.key` secret, and back it up: without it, every node has to join
again.

## [node]

With `role = "node"`: the hub this node connects to. `watcher node join`
writes it.

| Key | Default | Description |
|---|---|---|
| `hub` | | Required. The hub's address for nodes, for example `https://hub.example.com:7443`. Without a port, 7443. |

The node keeps its key and certificates in `data_dir/node/`. A node
ignores the other settings except `data_dir`.

## [tls]

HTTPS for the web UI and API. Set `cert` and `key`, `self_signed`, or
[`[tls.acme]`](#tlsacme) for certificates from Let's Encrypt.

| Key | Default | Description |
|---|---|---|
| `cert` | | PEM file with the certificate chain. |
| `key` | | PEM file with the private key. |
| `self_signed` | `false` | `true`: Watcher creates a certificate in `data_dir/tls/` for localhost, the hostname, the listen IP and the `public_url` host. |

```toml
[tls]
self_signed = true
```

Alternatively, run Watcher behind a reverse proxy, see
[HTTPS](../install.md#https).

### [tls.acme]

Certificates from Let's Encrypt or another CA that speaks ACME. Watcher
orders one when it starts without one, and a new one when a third of its
lifetime is left (at most 30 days before it ends). To check each name,
the CA connects to port 443 of the name, which has to reach Watcher. Step
by step: [Certificates from Let's Encrypt](../install.md#certificates-from-lets-encrypt).

| Key | Default | Description |
|---|---|---|
| `domains` | the host of `public_url` | Names the certificate is for. IP addresses don't work. |
| `email` | | Address the CA writes to about expiring certificates and problems. |
| `directory` | Let's Encrypt | ACME directory URL of the CA. Let's Encrypt's test CA: `https://acme-staging-v02.api.letsencrypt.org/directory`. |
| `ca` | | PEM file with the root certificate of the CA's own HTTPS, if the system doesn't trust it (internal CAs). Relative paths are resolved like `conf_dir`. |
| `eab_kid` | | External account binding, for CAs that require it (ZeroSSL, Google Trust Services): the key ID from the CA. Needs `eab_hmac_key`. |
| `eab_hmac_key` | | The HMAC key that goes with `eab_kid`, base64url as the CA shows it. |

```toml
public_url = "https://watcher.example.com"

[tls.acme]
email = "ops@example.com"
```

`[tls.acme]` can't be combined with `cert`, `key` or `self_signed`. The
account, the certificate and its key are kept in `data_dir/acme/`.

## [smtp]

The mail server for email notifications.

| Key | Default | Description |
|---|---|---|
| `host` | | Required. Mail server. |
| `port` | `587`, `465` or `25` | Depends on `tls`: 587 for `starttls`, 465 for `tls`, 25 for `none`. |
| `tls` | `starttls` | `starttls`: plain connection upgraded to TLS; TLS is required. `tls`: TLS from the start (SMTPS). `none`: no encryption. |
| `username` | | Login user. |
| `password` | | Login password. |
| `password_file` | | File with the login password, instead of `password`. |
| `from` | | Required. Sender, `Name <address>` or `address`. |

A local relay:

```toml
[smtp]
host = "localhost"
tls = "none"
from = "watcher@example.com"
```
