# Command line

```
watcher <command> [options]
```

`PATH` arguments: a `watcher.toml`, or a directory of object files
(no bootstrap file). Default: `./watcher.toml`, then
`/etc/watcher/watcher.toml`.

Log level: `RUST_LOG`, e.g. `RUST_LOG=debug watcher run`.

## run

Run all checks, serve the web UI and API.

```
watcher run [PATH] [options]
```

| Option | Default | |
|---|---|---|
| `--all` | | log every result, not only state changes |
| `--max-concurrent N` | 64 | checks running at the same time |
| `--database URL` | `[database]` | env `WATCHER_DATABASE_URL` |
| `--no-database` | | don't store results |
| `--no-auth` | | no login; loopback only |
| `--listen ADDR` | `listen` | e.g. `0.0.0.0:8080` |

A `PATH` to a `watcher.toml` that doesn't exist starts the browser setup.
`SIGHUP` reloads the object files.

```sh
watcher run /etc/watcher/watcher.toml
watcher run examples/demo/watcher.toml --database postgres://watcher:watcher@localhost/demo
```

## run-check

Run one command, print the evaluated result. Exit code: final state
(0 OK, 1 WARNING, 2 CRITICAL, 3 UNKNOWN).

```
watcher run-check [options] [--] COMMAND...
```

| Option | Default | |
|---|---|---|
| `--format F` | `auto` | `auto`, `nagios`, `key_value`, `json` |
| `--timeout D` | `60s` | |
| `--shell` | | run with `/bin/sh -c` |
| `--warn METRIC=THRESHOLD` | | repeatable |
| `--crit METRIC=THRESHOLD` | | repeatable |
| `--output-limit BYTES` | 65536 | per stream |
| `--json` | | result as JSON |

Options go before the command. One argument is split like a shell would;
several are used as they are. Bare names are found in the default check
directories.

```sh
watcher run-check -- check_disk -w 20% -c 10% -p /
watcher run-check --shell 'echo "users=$(who | wc -l)"'
watcher run-check --crit 'time=> 2s' -- check_http -H example.com
```

## init

Write `watcher.toml` and `conf.d/` from questions in the terminal.

```
watcher init [DIR] [--answers FILE] [--force]
```

| Option | |
|---|---|
| `DIR` | default `.` |
| `--answers FILE` | no questions; format: [examples/init-answers.toml](../../examples/init-answers.toml) |
| `--force` | overwrite existing files |

Piped stdin works too.

## setup

Setup wizard in the browser, then run.

```
watcher setup [DIR] [--listen ADDR]
```

| Option | Default | |
|---|---|---|
| `DIR` | `.` | |
| `--listen ADDR` | `127.0.0.1:8080` | `0.0.0.0:8080` for a remote machine |

The setup token is printed to the log.

## config check

Validate; summary or every service.

```
watcher config check [PATH] [--services]
```

```
OK: 4 files, 2 hosts, 2 roles, 4 checks, 5 services, 1 contact

  localhost/disk_root  every 30s    check_disk -w '20%' -c '10%' -p /
  localhost/backup     fresh 1day 2h  (passive)
```

Errors with file and line, exit code 1:

```
hosts.toml:4: error: host "h1": unknown role "b"
```

## config explain

Config Tree: where an object and its values come from, where it is used.

```
watcher config explain <host>[/<service>] [-c PATH]
watcher config explain <kind> <name> [-c PATH]
```

Kinds: `host`, `service`, `role`, `check`, `hostgroup`, `servicegroup`,
`contact`, `contactgroup`, `notify`.

```sh
watcher config explain web01
watcher config explain web01/http
watcher config explain role linux
watcher config explain notify dba-critical
```

## notify test

Send a test message to a contact over all its channels.

```
watcher notify test <contact> [PATH]
```

## user

Accounts, in the database.

```
watcher user add <name> [--admin] [--password-stdin]
watcher user passwd <name> [--password-stdin]
watcher user list
watcher user remove <name>
```

- `--admin`: may edit the configuration.
- Passwords: at least 10 characters. Asked twice, or first line of stdin.
- `passwd` ends the user's sessions; `remove` deletes sessions and tokens.

## token

API tokens, shown once.

```
watcher token create <user> <name>
watcher token list <user>
watcher token revoke <user> <name>
```

```sh
watcher token create alice "backup cron job"
```

## db reader

Read-only database login for Grafana, BI, SQL. Creates it or sets its
password. Needs CREATEROLE; rerun after upgrades.

```
watcher db reader <name> [--password-stdin]
```

Without `--password-stdin` a password is generated and printed.

## Database options

`user`, `token`, `db` take:

| Option | |
|---|---|
| `-c, --config FILE` | `watcher.toml` with `[database]` |
| `--database URL` | env `WATCHER_DATABASE_URL` |
