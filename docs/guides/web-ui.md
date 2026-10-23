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

Or in the web UI: Settings → Users (admins); everyone changes their own
password under Account.

Five failed logins lock a username for a minute. Sessions last 14 days
from the last use.

Without a database: no login, loopback only. `watcher run --no-auth`: no
login with a database, loopback only.

## Pages

| Page | |
|---|---|
| Tactical Overview | Hosts DOWN, service problems, totals |
| Hosts | host list, state, services per state |
| Host | services, commands, graphs, alert history |
| Services | all services, filter by state |
| Service | state, output, commands, performance data with thresholds, graphs, alert history |
| Problems | non-OK services |
| Host Groups, Service Groups | totals per group |
| Alerts | alert history: host, type, state, SOFT/HARD, time range |
| Downtimes | schedule and end downtimes; acknowledged, muted, checks off |
| Process Info | uptime, results/min; notifications and active checks on/off |
| Scheduling Queue | next check of every service |
| Configuration | forms and file editor (admins) |
| Account | user, password, API tokens |
| Users | add, admin, disable, password, delete (admins) |

Status totals are links to the filtered lists. Columns sort on click.
The Problems badge counts unhandled problems (not acknowledged, not in a
downtime).

**Commands:** right-click a host or service row, or use the Commands
panel on its page: Re-Check Now, Acknowledge Problem, Schedule Downtime,
Mute Notifications, Disable Active Checks, Submit Result (passive), and
their undo. Badges: ACK, DOWNTIME, MUTED, CHECKS OFF.
[Operations](operations.md).

**Live / Paused** (top bar): live updates over Server-Sent Events. Paused:
pages hold still; Refresh loads once.

**Graphs:** 15 min by default, ranges from 30 s; drag to zoom,
double-click to reset. Host page: the `graph` service, or the host check,
with a picker. `graphs = false` on a check or service: no graphs.

## Configuration

Admins. Menu: Checks, Services, Hosts, Host Groups, Service Groups,
Contacts, Contact Groups, Downtime Groups, Notifications, Files.

- Lists show file and line of every object.
- Forms show the known fields; other keys in the file are kept. An empty
  field removes the key.
- Text fields suggest values (objects, scripts, vars, metrics, durations);
  any value can be typed.
- Host form: host groups, services, service groups, overrides of
  assigned services, one-off services.
- Group membership shows on both sides: the host form lists host groups
  that list the host (`Also: dc1 · pattern web*`), the host group form
  hosts that list the group; the same for service and contact groups.
  × removes it there on save. Deleting an object takes it out of those
  lists too. Lists show it muted.
- Selectors, host group members and downtime groups show what they
  match now: `3 hosts: web01 (name web*), …`; errors (e.g. in `expr`)
  at once.
- **Config Tree:** where the object and each value come from, where it is
  used. Each entry links to its form or file line.
- **Save** → preview of the effective changes (+ added, ~ changed,
  − removed) and problems → **Apply**. Nothing is written before Apply.
- New objects go to `conf.d/ui/` (`checks.toml`, `services.toml`,
  `hosts.toml`, `groups.toml`, `contacts.toml`, `notify.toml`). Edits of existing objects
  stay in their file; comments and formatting are kept.
- Objects written as inline arrays can't be edited in forms; use Files.

**Clone:** a new form filled from the object. Several names at once:
`web03, web[04-10]` (8 clones, in the original's file). Cloned hosts get
their address from the name.

**Edit Config** (status pages): host → host form or its host groups;
service → service, host override or check; group → group form.

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
