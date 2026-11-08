# downtimegroup

A downtime group is a set of hosts that share recurring downtimes, for
example a weekly patch window. During a window, the hosts and their
services send no notifications.

```toml
[downtimegroup.patchday]
title = "Patch Day"
hostgroups = ["linux"]
schedule = [{ days = ["sun"], from = "02:00", to = "04:00" }]
```

Every Sunday from 02:00 to 04:00, the hosts of the host group `linux` and
all their services are in a downtime.

One-off downtimes, for example for maintenance tonight, are not written
in the configuration. Schedule them in the web UI or over the API, see
[Downtimes](../guides/operations.md#downtimes).

## Fields

| Field | Default | Description |
|---|---|---|
| `title` | | Name shown in the web UI. |
| `members` | | Host names. `*` matches any text, `?` one character. |
| `hostgroups` | | Host groups; all their hosts are members. |
| `schedule` | | List of windows, see [Windows](#windows). Without a schedule, the group is only a target for one-off downtimes. |

### Windows

Each window is a table `{ days, from, to }`. Times are local time; the time
zone is `timezone` in [watcher.toml](../reference/watcher-toml.md), or the
system's.

| Field | Default | Description |
|---|---|---|
| `days` | every day | Weekdays the window starts on: `mon`, `tue`, `wed`, `thu`, `fri`, `sat`, `sun`. |
| `from` | | Required. Start time, `HH:MM`. Or a date and time, `YYYY-MM-DD HH:MM`, for a window that happens once. |
| `to` | | Required. End time, `HH:MM`. If it is not after `from`, the window ends on the next day. With a date in `from`, `to` needs a date too. |

## Examples

### Every Sunday night

```toml
[downtimegroup.patchday]
members = ["web*", "db01"]
schedule = [{ days = ["sun"], from = "02:00", to = "04:00" }]
```

### Every night

```toml
[downtimegroup.backup-window]
hostgroups = ["fileservers"]
schedule = [{ from = "01:00", to = "03:00" }]
```

### Across midnight

The window starts Saturday at 22:00 and ends Sunday at 02:00.

```toml
[downtimegroup.weekend]
members = ["batch*"]
schedule = [{ days = ["sat"], from = "22:00", to = "02:00" }]
```

### Once, on fixed dates

```toml
[downtimegroup.holidays]
hostgroups = ["office"]
schedule = [{ from = "2026-12-24 00:00", to = "2026-12-27 00:00" }]
```

### Several windows

```toml
[downtimegroup.patchday]
title = "Patch Day"
members = ["web*", "db01"]
hostgroups = ["linux"]
schedule = [
  { days = ["sun"], from = "02:00", to = "04:00" },
  { days = ["sat"], from = "22:00", to = "02:00" },
  { from = "2026-12-24 00:00", to = "2026-12-27 00:00" },
]
```

### Only a target for one-off downtimes

A group without a schedule can be chosen in Schedule Downtime → Downtime
Group, so you can put the same set of hosts into a downtime with one
click.

```toml
[downtimegroup.storage]
title = "Storage Cluster"
members = ["san*", "nas*"]
```

## Behaviour

- A downtime start and end can notify (DOWNTIMESTART, DOWNTIMEEND); see
  [notify](notify.md#on).
- A host in a downtime counts once, not once per service.
- System → Downtimes in the web UI shows the current or next window of
  each downtime group.

## See also

- [Operations](../guides/operations.md): one-off downtimes, acknowledgements, muting
