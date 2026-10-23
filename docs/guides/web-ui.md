# Web UI

Built into the binary. Default http://127.0.0.1:8080 (`listen` in
[watcher.toml](../reference/watcher-toml.md)).

## Login

With a database: login required. Accounts:

```sh
watcher user add alice              # asks for the password
watcher user add admin --admin      # may edit the configuration
watcher user passwd alice
```

Five failed logins lock a username for a minute. Sessions last 14 days
from the last use.

Without a database: no login, loopback only. `watcher run --no-auth`: no
login with a database, loopback only.

## Pages

| Page | |
|---|---|
| Tactical Overview | Hosts DOWN, service problems, totals |
| Hosts | host list, state, services per state |
| Host | services, graphs, alert history |
| Services | all services, filter by state |
| Service | state, output, performance data with thresholds, graphs, alert history |
| Problems | non-OK services |
| Host Groups, Service Groups | totals per group |
| Alerts | alert history: host, type, state, SOFT/HARD, time range |
| Configuration | forms and file editor (admins) |
| Account | user, API tokens |

Status totals are links to the filtered lists. Columns sort on click.

**Live / Paused** (top bar): live updates over Server-Sent Events. Paused:
pages hold still; Refresh loads once.

**Graphs:** ranges from 30 s; drag to zoom, double-click to reset. Host
page: the `graph` service, or the host check, with a picker.

## Configuration

Admins. Menu: Hosts, Roles, Checks, Host Groups, Service Groups, Contacts,
Contact Groups, Notifications, Files.

- Lists show file and line of every object.
- Forms show the known fields; other keys in the file are kept. An empty
  field removes the key.
- Text fields suggest values (objects, tags, scripts, vars, metrics,
  durations); any value can be typed.
- Host form: roles, groups, own services, overrides of role services.
- **Config Tree:** where the object and each value come from, where it is
  used. Each entry links to its form or file line.
- **Save** → preview of the effective changes (+ added, ~ changed,
  − removed) and problems → **Apply**. Nothing is written before Apply.
- New objects go to `conf.d/ui/` (`hosts.toml`, `roles.toml`,
  `checks.toml`, `groups.toml`, `contacts.toml`, `notify.toml`). Edits of existing objects
  stay in their file; comments and formatting are kept.
- Objects written as inline arrays can't be edited in forms; use Files.

**Clone:** a new form filled from the object. Several names at once:
`web03, web[04-10]` (8 clones, in the original's file). Cloned hosts get
their address from the name.

**Edit Config** (status pages): host → host form; service → role entry,
host override or check; group → group form.

**Files:** list, new file, editor with live validation (markers at the
line, summary of the effective changes).

**Conflicts:** a file changed on disk since it was opened: the editor
shows the differences: revert per chunk, Use Disk Version, Keep Mine.

## Config mode

`config_mode` in `watcher.toml`:

| Mode | UI | Files |
|---|---|---|
| `hybrid` (default) | edit | watched, reloaded within a second |
| `files` | read-only | watched |
| `ui` | edit | not watched; `SIGHUP` reloads |

A config with errors is never applied: the running one stays, the log and
the UI say why.

## API tokens

Account → API Tokens → Create token. Shown once. For scripts and passive
results: `Authorization: Bearer wt_...`. Same with
`watcher token create <user> <name>`.

## Development

```sh
npm --prefix web run dev       # proxies /api to 127.0.0.1:8080
```

Debug builds read `web/build` from disk; release builds embed it.
