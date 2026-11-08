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
| `role` | `standalone` | `standalone`, `hub` or `node`. *Not yet:* `hub` and `node` behave like `standalone`. |
| `instance_name` | | Name of this installation, shown in the sidebar, the page title and the mail subject. |
| `timezone` | the system's | IANA time zone name, like `Europe/Berlin`. Used for downtime group windows. |
| `listen` | `127.0.0.1:8080` | Address and port of the web UI and API. Any address other than loopback needs a database, because logins need one. |
| `public_url` | | The URL users open Watcher with. Used for links in notifications. With `https://`, session cookies are marked Secure. |
| `conf_dir` | `conf.d` | Directory with the object files. Every `*.toml` below it is read. |
| `data_dir` | `data` | Directory for sink buffers, the instance ID, the self-signed certificate and `secret.key` (see [SSH secrets](../guides/ssh.md#secrets)). |
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

## [tls]

HTTPS for the web UI and API. Set `cert` and `key`, or `self_signed`.

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
