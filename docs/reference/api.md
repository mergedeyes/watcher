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
`config/*`: admins only.

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
| `GET groups` | host and service groups |
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

Cookie requests that change something need a matching `Origin`.

## Configuration

Admins.

| Method, path | |
|---|---|
| `GET config` | mode, files with hashes, problems on disk |
| `GET config/files/{path}` | one file |
| `GET config/objects` | objects as written, with file and line |
| `GET config/scripts` | check directories and their executables |
| `GET config/tree/{kind}/{name}` | Config Tree; service name `host/service` |
| `POST config/edit` | object → file change; writes nothing |
| `POST config/edit-many` | several, e.g. clones |
| `POST config/preview` | validate changes, effective changes |
| `POST config/apply` | write and reload |

Flow: `edit` → `preview` → `apply`.

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
