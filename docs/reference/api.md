# HTTP API

JSON under `/api/v1`. The web UI uses only this API. Full spec: OpenAPI
3.1 at `/api/v1/openapi.yaml` ([crates/hub/openapi.yaml](../../crates/hub/openapi.yaml)).

## Authentication

| | |
|---|---|
| Scripts | `Authorization: Bearer wt_...` ([token](cli.md#token) or Account page) |
| Browser | session cookie from `POST auth/login` |
| No login | without a database, or `--no-auth` (loopback only) |

Public: UI files, `health`, `openapi.yaml`, `auth/status`, `auth/login`.
Admins only: `config/*`, `users` (except your own password), `PUT
process`, `run-check`. Everything else: any logged-in user.

Everything the web UI does is in the API; a token has the rights of its
user.

```sh
export T=wt_...
curl -H "Authorization: Bearer $T" http://127.0.0.1:8080/api/v1/services?problems=true
```

Errors: `{"error": "..."}` with 400, 401, 403, 404, 409, 422, 503 (needs a
database).

## Status

| Method, path | |
|---|---|
| `GET health` | status, version |
| `GET hosts` | hosts with state |
| `GET hosts/{host}` | host with its services |
| `GET services` | services; filters below |
| `GET services/{host}/{service}` | service with last result |
| `POST services/{host}/{service}/result` | passive result |
| `POST services/{host}/{service}/run` | run its command once, not recorded |
| `GET groups` | host, service and downtime groups |
| `GET stream` | live updates, Server-Sent Events |

`GET services` filters:

| Parameter | |
|---|---|
| `host` | host name |
| `state` | comma list: `ok,warning,critical,unknown,pending` |
| `problems=true` | not OK, not PENDING |
| `hard=true` | HARD states only |

```sh
curl -H "Authorization: Bearer $T" '.../api/v1/services?state=critical,unknown&hard=true'
```

Passive result:

```sh
curl -X POST -H "Authorization: Bearer $T" -H 'content-type: application/json' \
  -d '{"exit_code": 1, "output": "BACKUP WARNING - 2 files skipped | skipped=2"}' \
  .../api/v1/services/backup01/backup/result
```

Body: `exit_code` (0–3) or `state` (`ok`, `warning`, `critical`,
`unknown`); `output` parsed like stdout.

Stream events: `result` (service after every check), `reload` (config
reloaded, fetch lists again), `lagged` (updates dropped, fetch again).

```sh
curl -N -H "Authorization: Bearer $T" .../api/v1/stream
```

Service and host views carry `acknowledged`, `muted`, `checks_enabled`
and `downtime`.

## Commands

`POST actions/{action}`:

| Action | |
|---|---|
| `recheck` | run now (also while checks are off) |
| `ack`, `unack` | acknowledge a problem; ends when OK |
| `mute`, `unmute` | notifications off/on |
| `disable-checks`, `enable-checks` | active checks off/on |
| `downtime` | no notifications from `start` to `end`, or for `duration` |

Body: targets `hosts`, `services` (`host/service`), `hostgroups`,
`servicegroups`, `downtimegroups`; `comment`; for `downtime` `start`
(default now), `end` or `duration` (`2h`). A host covers its services;
`ack` on a host acknowledges its host check. Groups are expanded, except
in downtimes: those keep the group, later members are covered too.
Response: `done`, `skipped` (with reason), `downtimes`.

```sh
curl -X POST -H "Authorization: Bearer $T" -H 'content-type: application/json' \
  -d '{"hosts": ["web01"], "duration": "2h", "comment": "patching"}' .../api/v1/actions/downtime
curl -X POST -H "Authorization: Bearer $T" -H 'content-type: application/json' \
  -d '{"services": ["db01/replication"], "comment": "on it"}' .../api/v1/actions/ack
curl -X POST -H "Authorization: Bearer $T" -H 'content-type: application/json' \
  -d '{"hostgroups": ["linux"]}' .../api/v1/actions/recheck
```

| Method, path | |
|---|---|
| `GET flags` | acknowledgements, muted, checks off |
| `GET downtimes` | downtimes not ended, and downtime group windows |
| `DELETE downtimes/{id}` | end or cancel a downtime |
| `GET process` | process info: version, uptime, switches, scheduler, results/min |
| `PUT process` | `{"notifications": bool, "checks": bool}` (admins) |
| `GET scheduler` | scheduling queue, soonest first; `limit` |
| `POST run-check` | `{"command", "shell", "format", "timeout"}`: run as a check (admins) |

```sh
curl -X PUT -H "Authorization: Bearer $T" -H 'content-type: application/json' \
  -d '{"notifications": false}' .../api/v1/process
curl -X POST -H "Authorization: Bearer $T" -H 'content-type: application/json' \
  -d '{"command": "check_ping -H 10.0.0.5 -w 100,20% -c 500,60%"}' .../api/v1/run-check
```

## History and metrics

Need a database.

| Method, path | |
|---|---|
| `GET events` | alert history, newest first |
| `GET services/{host}/{service}/series` | metric series of a service |
| `GET series/{id}/points` | points, bucketed |

`GET events`:

| Parameter | |
|---|---|
| `host`, `service` | |
| `type` | comma list: `service,host,flapping` |
| `state` | comma list: `ok,warning,critical,unknown,up,down` |
| `state_type` | `soft`, `hard` |
| `hard=true` | confirmed problems and their recoveries |
| `since`, `before` | time |
| `limit` | |

`GET series/{id}/points`:

| Parameter | Default | |
|---|---|---|
| `from` | `1h` | time |
| `to` | now | time |
| `points` | 600 | max buckets, 1–10000 |

Times: RFC 3339, unix seconds, or a duration back from now (`6h`, `7d`).

```sh
curl -H "Authorization: Bearer $T" '.../api/v1/events?host=web01&hard=true&since=7d'
curl -H "Authorization: Bearer $T" '.../api/v1/series/42/points?from=2026-10-01T00:00:00Z&to=2026-10-02T00:00:00Z'
```

## Auth

| Method, path | |
|---|---|
| `GET auth/status` | login needed, current user |
| `POST auth/login` | `{"username", "password"}`; sets the cookie. 429 when locked |
| `POST auth/logout` | |
| `GET tokens` | own tokens |
| `POST tokens` | `{"name"}` → `{"token": "wt_...", "info"}`, shown once |
| `DELETE tokens/{id}` | revoke |
| `GET users` | accounts (admins) |
| `POST users` | `{"username", "password", "admin"}` (admins) |
| `PUT users/{name}` | `{"password", "admin", "disabled"}`; own password for everyone |
| `DELETE users/{name}` | with sessions and tokens (admins, not yourself) |

Passwords: at least 10 characters. A new password or disabling ends the
user's sessions.

Cookie requests that change something need a matching `Origin`.

## Configuration

Admins.

| Method, path | |
|---|---|
| `GET config` | mode, files with hashes, problems on disk |
| `GET config/files/{path}` | one file |
| `GET config/objects` | objects as written, with file and line; `kind` filter |
| `GET config/objects/{kind}/{name}` | one object |
| `PUT config/objects/{kind}/{name}` | create or replace it, apply |
| `DELETE config/objects/{kind}/{name}` | delete it, apply |
| `POST config/reload` | read the files again |
| `POST config/match` | `{"kind": "host"\|"service", "selectors": [...]}`: what they match now, with why |
| `GET config/scripts` | check directories and their executables |
| `GET config/tree/{kind}/{name}` | Config Tree; service name `host/service` |
| `POST config/edit` | object → file change; writes nothing |
| `POST config/edit-many` | several, e.g. clones |
| `POST config/preview` | validate changes, effective changes |
| `POST config/apply` | write and reload |

One call:

```sh
curl -X PUT -H "Authorization: Bearer $T" -H 'content-type: application/json' \
  -d '{"address": "10.0.0.7", "groups": ["linux"]}' .../api/v1/config/objects/host/web03
curl -X PUT -H "Authorization: Bearer $T" -H 'content-type: application/json' \
  -d '{"check": "disk", "vars": {"path": "/srv"}}' .../api/v1/config/objects/service/disk_srv
curl -X DELETE -H "Authorization: Bearer $T" .../api/v1/config/objects/host/web03
```

`PUT` body: the fields as written, without `name`. `?path=hosts/web.toml`
puts a new object into that file. Kinds: `check`, `service`, `host`,
`hostgroup`, `servicegroup`, `contact`, `contactgroup`, `downtimegroup`,
`notify`, `sink`.

With a preview first: `edit` → `preview` → `apply`.

```sh
# 1. file change for a new host
curl -X POST -H "Authorization: Bearer $T" -H 'content-type: application/json' \
  -d '{"kind": "host", "fields": {"name": "web03", "address": "10.0.0.7", "groups": ["linux"]}}' \
  .../api/v1/config/edit
# → {"path": "ui/hosts.toml", "base": "…", "text": "…"}

# 2. apply it
curl -X POST -H "Authorization: Bearer $T" -H 'content-type: application/json' \
  -d '{"changes": [{"path": "ui/hosts.toml", "base": "…", "text": "…"}]}' \
  .../api/v1/config/apply
```

Edit body: `kind`, `name` (null: create), `fields` (null: delete),
`path` (new objects; default `ui/hosts.toml`, `ui/services.toml`,
`ui/checks.toml`, `ui/groups.toml`, `ui/contacts.toml`, `ui/notify.toml`,
`ui/sinks.toml`).

Change: `path` (relative, `.toml`, no hidden or `..` parts), `base` (file
hash; null: new file), `text` (null: delete file).

| Status | |
|---|---|
| 409 | file changed since `base`; body has the current file |
| 422 | config would have errors; diagnostics with file and line, nothing written |
| 403 | `config_mode = "files"` |
