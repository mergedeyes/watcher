# HTTP API

The API speaks JSON under `/api/v1`. The web UI uses nothing but this API,
so everything the web UI can do, a script can do too. The full
specification is OpenAPI 3.1 at `/api/v1/openapi.yaml`
([crates/hub/openapi.yaml](../../crates/hub/openapi.yaml)).

Paths below are relative to `/api/v1`.

## Authentication

| Client | How it logs in |
|---|---|
| Scripts | `Authorization: Bearer wt_...` header with an API token (see [token](cli.md#token), or the Account page). |
| Browser | Session cookie from `POST auth/login`. |
| No login | Without a database, or with `--no-auth`. Only on loopback. |

A token has the same rights as its user.

| Access | Endpoints |
|---|---|
| Public | UI files, `health`, `openapi.yaml`, `auth/status`, `auth/login` |
| Admins only | `config/*`, `ssh/*`, `users` (except changing your own password), `PUT process`, `run-check` |
| Any logged-in user | everything else |

```sh
export T=wt_...
curl -H "Authorization: Bearer $T" http://127.0.0.1:8080/api/v1/services?problems=true
```

Errors come as `{"error": "..."}` with status 400, 401, 403, 404, 409, 422,
423, or 503 (the endpoint needs a database).

Requests with a session cookie that change something need a matching
`Origin` header.

## Status

| Method and path | Description |
|---|---|
| `GET health` | Status and version. |
| `GET hosts` | Hosts with their state; filters below. |
| `GET hosts/{host}` | One host with its services. |
| `GET services` | Services; filters below. |
| `GET services/{host}/{service}` | One service with its last result. |
| `POST services/{host}/{service}/result` | Submit a result for a passive service. |
| `POST services/{host}/{service}/run` | Run the service's command once. The result is shown, not recorded. |
| `GET groups` | Host groups, service groups and downtime groups, with the number of hosts and services in each state. |
| `GET summary` | The number of hosts and services in each state, and how many problems are unhandled. |
| `GET names` | All host names and the service names, for suggestions. |
| `GET stream` | Live updates as Server-Sent Events. |

Filters for `GET services`:

| Parameter | Description |
|---|---|
| `host` | Only services of this host. |
| `hostgroup` | Only services of hosts in this host group. |
| `group` | Only services in this service group. |
| `state` | Comma-separated list: `ok,warning,critical,unknown,pending`. |
| `problems=true` | Only services that are not OK and not PENDING. |
| `hard=true` | Only HARD states. |
| `unhandled=true` | Only problems that are not acknowledged and not in a downtime. |
| `q` | Only services with this text in the host name, service name, title or output. Upper and lower case don't matter. |

Filters for `GET hosts`:

| Parameter | Description |
|---|---|
| `state` | Comma-separated list: `up,down,pending`. |
| `group` | Only hosts in this host group. |
| `unhandled=true` | Only DOWN hosts that are not acknowledged and not in a downtime. |
| `q` | Only hosts with this text in the name, address or host groups. |

Both lists can be sorted and split into pages:

| Parameter | Description |
|---|---|
| `sort` | Services: `host`, `service`, `status`, `last_check`, `duration` or `attempt`. Hosts: `host`, `status`, `address`, `last_check` or `duration`. Without it, the list is in config order. Names sort by their numbers: `web2` comes before `web10`. |
| `desc=true` | Reverse the order. |
| `offset` | Skip this many entries. |
| `limit` | Return at most this many entries. Without it, you get all of them. |

The answer has the page in `services` (or `hosts`), the number of
entries that match the filters in `total`, and `counts`: how many are in
each state. `counts` ignores the `state` filter, so it shows the other
states too.

```sh
curl -H "Authorization: Bearer $T" '.../api/v1/services?state=critical,unknown&hard=true'
curl -H "Authorization: Bearer $T" '.../api/v1/services?problems=true&sort=status&desc=true&limit=50'
# → {"counts": {"ok": 0, "warning": 3, …}, "total": 12, "services": [ …50 at most… ]}
```

Host and service objects include `acknowledged`, `muted`,
`checks_enabled` and `downtime`.

### Passive results

```sh
curl -X POST -H "Authorization: Bearer $T" -H 'content-type: application/json' \
  -d '{"exit_code": 1, "output": "BACKUP WARNING - 2 files skipped | skipped=2"}' \
  .../api/v1/services/backup01/backup/result
```

The body contains `exit_code` (0 to 3) or `state` (`ok`, `warning`,
`critical`, `unknown`), and `output`, which is read like the output of a
command. See [Passive checks](../objects/check.md#passive-checks).

### Live updates

```sh
curl -N -H "Authorization: Bearer $T" .../api/v1/stream
```

| Event | Meaning |
|---|---|
| `result` | A service after each check. |
| `summary` | The same as `GET summary`. Sent when the stream opens and after states change, at most once a second. Its `generation` number changes whenever a state, an acknowledgement, a downtime or the configuration changes. |
| `reload` | The configuration was reloaded, or a command changed something; fetch the lists again. |
| `lagged` | Updates were dropped because the client was too slow; fetch again. |

With many services, a client usually only wants results for what it
shows. Open the stream with `?watch=true`. The first event is `hello`
with the stream's `id`. Then tell the hub which services and hosts you
want results for; each call replaces the previous list:

```sh
curl -X PUT -H "Authorization: Bearer $T" -H 'content-type: application/json' \
  -d '{"services": ["web01/http"], "hosts": ["db01"]}' .../api/v1/stream/<id>
```

`hosts` means all services of those hosts. `summary`, `reload` and
`lagged` still come for everything. When `generation` changes, fetch your
list again: services may have moved into or out of it.

## Commands

`POST actions/{action}` runs a command on hosts or services:

| Action | Description |
|---|---|
| `recheck` | Run the check now, even while checks are turned off. |
| `ack`, `unack` | Acknowledge a problem, or remove the acknowledgement. An acknowledgement ends when the service is OK again. |
| `mute`, `unmute` | Turn notifications off or on. |
| `disable-checks`, `enable-checks` | Turn active checks off or on. |
| `downtime` | No notifications from `start` until `end`, or for `duration`. |

| Body field | Description |
|---|---|
| `hosts` | Host names. A host covers all its services; `ack` on a host acknowledges its host check. |
| `services` | Service IDs, `host/service`. |
| `hostgroups`, `servicegroups`, `downtimegroups` | Groups. Expanded to their members, except for downtimes: a downtime keeps the group, so hosts that join later are covered too. |
| `comment` | Text shown with the acknowledgement or downtime. |
| `start` | `downtime` only. Start time; default now. |
| `end`, `duration` | `downtime` only. End time, or a duration like `2h`. |

The response lists what was `done`, what was `skipped` (with the reason),
and the `downtimes` created.

```sh
curl -X POST -H "Authorization: Bearer $T" -H 'content-type: application/json' \
  -d '{"hosts": ["web01"], "duration": "2h", "comment": "patching"}' .../api/v1/actions/downtime
curl -X POST -H "Authorization: Bearer $T" -H 'content-type: application/json' \
  -d '{"services": ["db01/replication"], "comment": "on it"}' .../api/v1/actions/ack
curl -X POST -H "Authorization: Bearer $T" -H 'content-type: application/json' \
  -d '{"hostgroups": ["linux"]}' .../api/v1/actions/recheck
```

| Method and path | Description |
|---|---|
| `GET flags` | Acknowledgements, muted services, services with checks off. |
| `GET downtimes` | Downtimes that haven't ended, and the windows of downtime groups. |
| `DELETE downtimes/{id}` | End or cancel a downtime. |
| `GET process` | Process info: version, running time, global switches, scheduler, results per minute. |
| `PUT process` | `{"notifications": bool, "checks": bool}`: the global switches. Admins only. |
| `GET scheduler` | The scheduling queue, soonest first. `limit` limits the number of entries. |
| `POST run-check` | `{"command", "shell", "format", "timeout"}`: run a command like `watcher run-check`. Admins only. |

```sh
curl -X PUT -H "Authorization: Bearer $T" -H 'content-type: application/json' \
  -d '{"notifications": false}' .../api/v1/process
curl -X POST -H "Authorization: Bearer $T" -H 'content-type: application/json' \
  -d '{"command": "check_ping -H 10.0.0.5 -w 100,20% -c 500,60%"}' .../api/v1/run-check
```

## History and metrics

These endpoints need a database.

| Method and path | Description |
|---|---|
| `GET events` | Alert history, newest first. |
| `GET notifications` | The notification log, newest first: one entry per contact and channel, with `error` if delivery failed. |
| `GET services/{host}/{service}/series` | The metric series of a service, with `warn` and `crit`: the threshold bounds of the latest result. |
| `GET series/{id}/points` | The points of a series, grouped into buckets. |

Parameters for `GET events`:

| Parameter | Description |
|---|---|
| `host`, `service` | Only events of this host or service. |
| `type` | Comma-separated list: `service,host,flapping,ack,downtime`. |
| `state` | Comma-separated list: `ok,warning,critical,unknown,up,down`. |
| `state_type` | `soft` or `hard`. |
| `hard=true` | Only confirmed problems and their recoveries. |
| `since`, `before` | Time range. |
| `limit` | Maximum number of events. |

Parameters for `GET notifications`:

| Parameter | Description |
|---|---|
| `host`, `service` | Only notifications about this host or service. For host notifications, `service` is the host check. |
| `contact` | Only notifications to this contact. |
| `host_check` | `true`: only host notifications. `false`: only service notifications. |
| `failed=true` | Only failed deliveries. |
| `since`, `before` | Time range. |
| `limit` | Maximum number of entries (default 100). |

Parameters for `GET series/{id}/points`:

| Parameter | Default | Description |
|---|---|---|
| `from` | `1h` | Start time. |
| `to` | now | End time. |
| `points` | `600` | Maximum number of buckets, 1 to 10000. |

Times can be RFC 3339, unix seconds, or a duration back from now (`6h`,
`7d`).

```sh
curl -H "Authorization: Bearer $T" '.../api/v1/events?host=web01&hard=true&since=7d'
curl -H "Authorization: Bearer $T" '.../api/v1/series/42/points?from=2026-10-01T00:00:00Z&to=2026-10-02T00:00:00Z'
```

## Users and tokens

| Method and path | Description |
|---|---|
| `GET auth/status` | Whether a login is needed, and the current user. |
| `POST auth/login` | `{"username", "password", "code"}`. Sets the session cookie. `code` is only for users with two-factor authentication: a code from their app or a recovery code. Without it, or with a wrong one, the answer is 401 with `"totp_required": true`. 429 when the username is locked. |
| `POST auth/logout` | Log out. |
| `GET auth/totp` | `{"enabled", "recovery_codes"}`: whether your two-factor authentication is on, and how many recovery codes are left. |
| `POST auth/totp/setup` | A new secret for the authenticator app: `{"secret", "uri", "qr"}`. `uri` is the `otpauth://` link, `qr` the same as an SVG image. Nothing is stored yet. |
| `PUT auth/totp` | `{"secret", "code"}` turns two-factor authentication on, once the code shows that the app is set up right. Answers `{"recovery_codes": [...]}`, shown only this once. |
| `DELETE auth/totp` | `{"code"}` turns it off. The code is from the app or a recovery code. |
| `POST auth/totp/recovery` | `{"code"}` → `{"recovery_codes": [...]}`: ten new recovery codes. The old ones stop working. |
| `GET tokens` | Your API tokens. |
| `POST tokens` | `{"name"}` → `{"token": "wt_...", "info"}`. The token is shown only once. |
| `DELETE tokens/{id}` | Revoke a token. |
| `GET users` | All accounts. Admins only. |
| `POST users` | `{"username", "password", "admin"}`. Admins only. |
| `PUT users/{name}` | `{"password", "admin", "disabled", "totp"}`. Everyone may change their own password. `"totp": false` turns a user's two-factor authentication off (admins). |
| `DELETE users/{name}` | Delete an account with its sessions and tokens. Admins only, not yourself. |

Passwords need at least 10 characters. A new password, or disabling an
account, logs the user out everywhere.

Each two-factor code works once. Wrong codes count like wrong passwords:
after five, the username is locked for a minute. See
[Two-factor authentication](../guides/web-ui.md#two-factor-authentication).

## SSH

Admins only. The values of secrets never leave the server.

| Method and path | Description |
|---|---|
| `GET ssh/secrets` | Name, kind, public key, fingerprint, and the hosts using each secret. |
| `POST ssh/secrets` | Create a secret, see below. `"replace": true` overwrites one of the same name. |
| `DELETE ssh/secrets/{name}` | Delete a secret. |
| `GET ssh/hostkeys` | Stored host keys, and keys that changed (`offered`). |
| `POST ssh/hostkeys/{address}/{port}/accept` | Accept a changed host key. |
| `DELETE ssh/hostkeys/{address}/{port}` | Forget a host key; the next key seen is stored. |

Bodies for `POST ssh/secrets`:

| Body | Creates |
|---|---|
| `{"name", "kind": "generate"}` | A new ed25519 key. |
| `{"name", "kind": "key", "private_key", "passphrase"}` | An imported key. |
| `{"name", "kind": "password", "password"}` | A password. |

```sh
curl -X POST -H "Authorization: Bearer $T" -H 'content-type: application/json' \
  -d '{"name": "monitor", "kind": "generate"}' .../api/v1/ssh/secrets
# → {"name": "monitor", "kind": "key", "public_key": "ssh-ed25519 AAAA... watcher-monitor", ...}
```

## Configuration

Admins only.

| Method and path | Description |
|---|---|
| `GET config` | Config mode, files with their hashes, who is editing them, problems on disk. |
| `GET config/files/{path}` | One file. |
| `GET config/objects` | All objects as written, with file and line. `kind` filters by kind. |
| `GET config/objects/{kind}/{name}` | One object. |
| `PUT config/objects/{kind}/{name}` | Create or replace an object, and apply. |
| `DELETE config/objects/{kind}/{name}` | Delete an object, and apply. |
| `POST config/reload` | Read the files again. |
| `POST config/match` | `{"kind": "host"\|"service", "selectors": [...]}`: what the selectors match now, and why. |
| `GET config/scripts` | The check directories and the executables in them. |
| `GET config/tree/{kind}/{name}` | The Config Tree. For a service on a host, the name is `host/service`. |
| `POST config/edit` | Turn an object change into a file change. Writes nothing. |
| `POST config/edit-many` | The same for several objects, for example clones. |
| `POST config/preview` | Validate file changes and show their effect. |
| `POST config/apply` | Write file changes and reload. |
| `GET config/locks` | Who is editing which file. |
| `PUT config/locks/{path}` | Take or renew the edit lease on a file. |
| `DELETE config/locks/{path}?lease=…` | Give the lease up. |

Kinds: `check`, `service`, `host`, `hostgroup`, `servicegroup`, `contact`,
`contactgroup`, `downtimegroup`, `timeperiod`, `notify`, `dependency`,
`sink`.

A notify rule or dependency without a `name` is addressed as `@path:line`, for example
`@ui/notify.toml:4` (URL-encoded in paths), the way `GET config/objects`
lists it.

### Changing one object

`PUT` takes the object's fields as they would be written in the file,
without `name`. `?path=hosts/web.toml` puts a new object into that file.

```sh
curl -X PUT -H "Authorization: Bearer $T" -H 'content-type: application/json' \
  -d '{"address": "10.0.0.7", "groups": ["linux"]}' .../api/v1/config/objects/host/web03
curl -X PUT -H "Authorization: Bearer $T" -H 'content-type: application/json' \
  -d '{"check": "disk", "vars": {"path": "/srv"}}' .../api/v1/config/objects/service/disk_srv
curl -X DELETE -H "Authorization: Bearer $T" .../api/v1/config/objects/host/web03
```

### With a preview first

`edit` → `preview` → `apply`, the way the web UI does it:

```sh
# 1. Get the file change for a new host
curl -X POST -H "Authorization: Bearer $T" -H 'content-type: application/json' \
  -d '{"kind": "host", "fields": {"name": "web03", "address": "10.0.0.7", "groups": ["linux"]}}' \
  .../api/v1/config/edit
# → {"path": "ui/hosts.toml", "base": "…", "text": "…"}

# 2. Apply it
curl -X POST -H "Authorization: Bearer $T" -H 'content-type: application/json' \
  -d '{"changes": [{"path": "ui/hosts.toml", "base": "…", "text": "…"}]}' \
  .../api/v1/config/apply
```

| Edit body field | Description |
|---|---|
| `kind` | Object kind. |
| `name` | Current name; `null` creates a new object. |
| `fields` | The object's fields; `null` deletes it. |
| `path` | File for new objects. Default: `ui/hosts.toml`, `ui/services.toml`, `ui/checks.toml`, `ui/groups.toml`, `ui/contacts.toml`, `ui/notify.toml`, `ui/dependencies.toml` or `ui/sinks.toml`, by kind. |

| Change field | Description |
|---|---|
| `path` | Relative path ending in `.toml`, with no hidden or `..` parts. |
| `base` | Hash of the file you started from; `null` for a new file. |
| `text` | New content; `null` deletes the file. |

| Status | Meaning |
|---|---|
| 409 | The file changed since `base`. The body contains the current file. |
| 422 | The configuration would have errors. The body lists them with file and line; nothing was written. |
| 403 | `config_mode = "files"`: the configuration is read-only. |
| 423 | Someone else is editing the file. See [Edit locks](#edit-locks). |

### Edit locks

While a file is open in the web UI, the page holds an **edit lease** on it.
Writes to that file from anyone else (`apply` and `PUT`/`DELETE` on
objects) get status 423 with who is editing it:

```json
{"error": "anna is editing hosts.toml in the web UI",
 "holder": {"path": "hosts.toml", "by": "anna", "via": "web", "since": "2026-11-09T09:42:00Z", "pid": 4711, "host": "mon01"}}
```

`via` is `web`, `cli` (`watcher config edit`), `vim`, `nano`, `emacs` or
`kate`. Files open in one of these editors on the server are locked the
same way.

A script can hold a lease too, so that nobody changes a file while it
works on it:

```sh
# Take the lease (renew it within 2 minutes with {"lease": "…"}, or it lapses)
curl -X PUT -H "Authorization: Bearer $T" -H 'content-type: application/json' \
  -d '{}' .../api/v1/config/locks/hosts.toml
# → {"lease": "6f26…", "path": "hosts.toml", "others": []}

# Write with the lease
curl -X POST -H "Authorization: Bearer $T" -H 'content-type: application/json' \
  -H 'X-Watcher-Lease: 6f26…' -d '{"changes": [...]}' .../api/v1/config/apply

# Give it up
curl -X DELETE -H "Authorization: Bearer $T" '.../api/v1/config/locks/hosts.toml?lease=6f26…'
```

`{"force": true}` takes the file over from whoever has it. A file open in
an editor can only be written with a lease taken this way; `others` then
lists the editors that still have it open.
