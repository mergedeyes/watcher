# watcher.toml

The bootstrap file: what is needed before the object files and the
database are read. Changes need a restart.

Found at: the path given, else `./watcher.toml`, else
`/etc/watcher/watcher.toml`. Relative paths inside are relative to this
file. Unknown keys are errors.

```toml
role = "standalone"
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

| Key | Default | |
|---|---|---|
| `role` | `standalone` | `standalone`, `hub`, `node` (*not yet*: hub/node behave as standalone) |
| `instance_name` | | shown in the sidebar, page title, mail subject |
| `timezone` | the system's | IANA name; downtime group windows |
| `listen` | `127.0.0.1:8080` | HTTP API and web UI. Non-loopback needs a database (logins) |
| `public_url` | | links in notifications; `https://` marks cookies Secure |
| `conf_dir` | `conf.d` | object files, every `*.toml` below |
| `data_dir` | `data` | sink buffers, instance id, self-signed certificate |
| `config_mode` | `hybrid` | `hybrid`: UI and files; `files`: UI read-only; `ui`: files not watched |
| `check_dirs` | `checks`, then the monitoring-plugins dirs | where bare command names are looked up, before `PATH` |

Default plugin dirs (those that exist): `/usr/lib/monitoring-plugins`,
`/usr/lib/nagios/plugins`, `/usr/lib64/nagios/plugins`,
`/usr/local/libexec/monitoring-plugins`, `/usr/local/nagios/libexec`.

## [database]

| Key | |
|---|---|
| `url` | `postgres://user:password@host[:port]/db` |

Overridden by `--database` / `WATCHER_DATABASE_URL`.

## [tls]

| Key | |
|---|---|
| `cert` | PEM certificate chain |
| `key` | PEM private key |
| `self_signed` | `true`: generate in `data_dir/tls/` for localhost, hostname, listen IP, `public_url` host |

`cert` and `key`, or `self_signed`.

## [smtp]

| Key | Default | |
|---|---|---|
| `host` | | required |
| `port` | 587 / 465 / 25 | by `tls` |
| `tls` | `starttls` | `starttls` (required), `tls` (SMTPS), `none` |
| `username` | | |
| `password` | | |
| `password_file` | | file with the password |
| `from` | | required, `Name <addr>` or `addr` |
