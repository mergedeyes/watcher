# Command line

```
watcher <command> [options]
```

Commands that take a `PATH` accept either a `watcher.toml`, or a directory
of object files (then there is no `watcher.toml`). Without a `PATH`,
Watcher uses `./watcher.toml`, then `/etc/watcher/watcher.toml`.

The log level is set with `RUST_LOG`, for example
`RUST_LOG=debug watcher run`.

## run

Runs all checks and serves the web UI and the API.

```
watcher run [PATH] [options]
```

| Option | Default | Description |
|---|---|---|
| `--all` | | Log every result, not only state changes. |
| `--max-concurrent N` | `64` | How many checks may run at the same time. |
| `--database URL` | `[database]` in `watcher.toml` | Database to use. Also: environment variable `WATCHER_DATABASE_URL`. |
| `--no-database` | | Run without a database; results are not stored. |
| `--no-auth` | | No login. Only listens on loopback. |
| `--listen ADDR` | `listen` in `watcher.toml` | Address to listen on, for example `0.0.0.0:8080`. |

If `PATH` points to a `watcher.toml` that doesn't exist yet, the setup
wizard starts in the browser. `SIGHUP` reloads the object files.

```sh
watcher run /etc/watcher/watcher.toml
watcher run examples/demo/watcher.toml --database postgres://watcher:watcher@localhost/demo
```

## run-check

Runs one command and shows how Watcher reads the result. Needs no
configuration. The exit code is the final state: 0 OK, 1 WARNING,
2 CRITICAL, 3 UNKNOWN.

```
watcher run-check [options] [--] COMMAND...
```

| Option | Default | Description |
|---|---|---|
| `--format F` | `auto` | Output format: `auto`, `nagios`, `key_value` or `json`. |
| `--timeout D` | `60s` | Kill the command after this time. |
| `--shell` | | Run the command with `/bin/sh -c`. |
| `--warn METRIC=THRESHOLD` | | WARNING threshold for a metric. Can be given several times. |
| `--crit METRIC=THRESHOLD` | | CRITICAL threshold for a metric. Can be given several times. |
| `--output-limit BYTES` | `65536` | Maximum output read per stream (stdout, stderr). |
| `--json` | | Print the result as JSON. |

Options go before the command. A single argument is split into words like
a shell would; several arguments are used as they are. Commands without a
path are found in the default check directories.

```sh
watcher run-check -- check_disk -w 20% -c 10% -p /
watcher run-check --shell 'echo "users=$(who | wc -l)"'
watcher run-check --crit 'time=> 2s' -- check_http -H example.com
```

See [Testing a command](../objects/check.md#testing-a-command).

## init

Asks questions in the terminal and writes `watcher.toml` and `conf.d/`.

```
watcher init [DIR] [--answers FILE] [--force]
```

| Option | Default | Description |
|---|---|---|
| `DIR` | `.` | Directory to write to. |
| `--answers FILE` | | Take the answers from a file instead of asking. Format: [examples/init-answers.toml](../../examples/init-answers.toml). |
| `--force` | | Overwrite existing files. |

Answers can also be piped to stdin.

## setup

Runs the setup wizard in the browser, then starts Watcher.

```
watcher setup [DIR] [--listen ADDR]
```

| Option | Default | Description |
|---|---|---|
| `DIR` | `.` | Directory to write to. |
| `--listen ADDR` | `127.0.0.1:8080` | Use `0.0.0.0:8080` to set up a remote machine. |

The setup token you need to enter on the page is printed to the log.

## config check

Checks the configuration and prints a summary, or every service with its
command.

```
watcher config check [PATH] [--services]
```

```
OK: 4 files, 2 hosts, 4 checks, 5 services, 5 on hosts, 1 contact

  localhost/disk_root  every 30s    check_disk -w '20%' -c '10%' -p /
  localhost/backup     fresh 1day 2h  (passive)
```

Errors are shown with file and line, and the exit code is 1:

```
hosts.toml:4: error: host "h1": unknown hostgroup "b"
```

## config explain

Shows the Config Tree: where an object and each of its values come from,
and where the object is used.

```
watcher config explain <host>[/<service>] [-c PATH]
watcher config explain <kind> <name> [-c PATH]
```

Kinds: `host`, `service`, `check`, `hostgroup`, `servicegroup`,
`contact`, `contactgroup`, `notify`. `service <name>` explains a service
definition and the hosts that have it; `<host>/<service>` explains the
service on one host.

```sh
watcher config explain web01
watcher config explain web01/http
watcher config explain service disk
watcher config explain hostgroup linux
watcher config explain notify dba-critical
```

See [Finding out where a value comes from](../guides/configuration.md#finding-out-where-a-value-comes-from).

## notify test

Sends a test message to a contact, on all its channels.

```
watcher notify test <contact> [PATH]
```

## user

Manages web UI accounts. Accounts are stored in the database.

```
watcher user add <name> [--admin] [--password-stdin]
watcher user passwd <name> [--password-stdin]
watcher user list
watcher user remove <name>
```

- `--admin`: the user may edit the configuration.
- Passwords need at least 10 characters. They are asked twice, or read
  from the first line of stdin with `--password-stdin`.
- `passwd` logs the user out everywhere. `remove` also deletes the user's
  sessions and API tokens.

## token

Manages API tokens. A token is shown only once, when it is created.

```
watcher token create <user> <name>
watcher token list <user>
watcher token revoke <user> <name>
```

```sh
watcher token create alice "backup cron job"
```

## db reader

Creates a read-only database login for Grafana, BI tools or SQL, or sets
its password. Watcher's database user needs the CREATEROLE right. Run it
again after upgrades.

```
watcher db reader <name> [--password-stdin]
```

Without `--password-stdin`, a password is generated and printed. See
[Read-only login](../guides/data.md#read-only-login).

## secret

Manages keys and passwords for [checks over SSH](../guides/ssh.md#secrets).
They are stored in the database, encrypted with `data_dir/secret.key`.

```
watcher secret new <name> [--replace]
watcher secret import <name> <file> [--passphrase-stdin] [--replace]
watcher secret password <name> [--password-stdin] [--replace]
watcher secret show <name>
watcher secret list
watcher secret remove <name>
```

- `new` creates an ed25519 key and prints only the public key to stdout,
  so you can append it directly: `watcher secret new monitor >> authorized_keys`.
- `import` reads ed25519 or ECDSA keys in OpenSSH, PEM or PuTTY format, and
  asks for the passphrase if the key is encrypted. RSA is not supported.
- `--replace` overwrites an existing secret of the same name.

## hostkey

Manages SSH host keys. Watcher stores a host's key on the first connection.

```
watcher hostkey list
watcher hostkey accept <address>[:<port>]
watcher hostkey forget <address>[:<port>]
```

See [Host keys](../guides/ssh.md#host-keys).

## Database options

`user`, `token`, `db`, `secret` and `hostkey` need the database. They find
it with these options:

| Option | Description |
|---|---|
| `-c, --config FILE` | A `watcher.toml` with `[database]`. |
| `--database URL` | The database URL. Also: environment variable `WATCHER_DATABASE_URL`. |
