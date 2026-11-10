# timeperiod

A time period says when something applies, for example business hours.
A [notify rule](notify.md) with `period` only notifies inside its time
period.

```toml
[timeperiod.workhours]
title = "Business hours"
schedule = [{ days = ["mon", "tue", "wed", "thu", "fri"], from = "08:00", to = "18:00" }]

[[notify]]
contact_groups = ["team"]
period = "workhours"
```

The team gets notifications on weekdays from 08:00 to 18:00. A problem
that starts at night and is still open at 08:00 is sent to them at 08:00.

## Fields

| Field | Default | Description |
|---|---|---|
| `title` | | Name shown in the web UI. |
| `schedule` | | Required. List of windows, see [Windows](#windows). The time period is active while any of them is. |

### Windows

Each window is a table `{ days, from, to }`. Times are local time; the time
zone is `timezone` in [watcher.toml](../reference/watcher-toml.md), or the
system's.

| Field | Default | Description |
|---|---|---|
| `days` | every day | Weekdays the window starts on: `mon`, `tue`, `wed`, `thu`, `fri`, `sat`, `sun`. |
| `from` | | Required. Start time, `HH:MM`. Or a date and time, `YYYY-MM-DD HH:MM`, for a window that happens once. |
| `to` | | Required. End time, `HH:MM`. If it is not after `from`, the window ends on the next day. With a date in `from`, `to` needs a date too. |

The windows are the same as in [downtime groups](downtimegroup.md#windows).

## Examples

### Business hours

```toml
[timeperiod.workhours]
schedule = [{ days = ["mon", "tue", "wed", "thu", "fri"], from = "08:00", to = "18:00" }]
```

### Different times on different days

A time period can have several windows, each with its own days and
times. It is active while any of them is.

```toml
[timeperiod.workhours]
title = "Business hours"
schedule = [
  { days = ["mon", "tue", "wed", "thu"], from = "08:00", to = "17:00" },
  { days = ["fri"], from = "08:00", to = "14:00" },
  { days = ["sat"], from = "10:00", to = "12:00" },
]
```

Monday to Thursday from 08:00 to 17:00, Friday until 14:00, Saturday from
10:00 to 12:00, and never on Sunday.

### Outside business hours

Weekday mornings and evenings, and the whole weekend. A `to` of
`"00:00"` means midnight at the end of the day, so `from = "00:00"` with
`to = "00:00"` is a full day.

Weekday windows from 18:00 to 08:00 together with full weekend days would
miss Monday 00:00 to 08:00: the Sunday window ends at midnight, and the
Monday night window only starts at 18:00. That is why the example splits
weekdays into a morning and an evening window.

```toml
[timeperiod.offhours]
schedule = [
  { days = ["mon", "tue", "wed", "thu", "fri"], from = "00:00", to = "08:00" },
  { days = ["mon", "tue", "wed", "thu", "fri"], from = "18:00", to = "00:00" },
  { days = ["sat", "sun"], from = "00:00", to = "00:00" },
]
```

### Weekends

From Saturday 00:00 to Monday 00:00.

```toml
[timeperiod.weekend]
schedule = [{ days = ["sat", "sun"], from = "00:00", to = "00:00" }]
```

### A holiday on-call shift

```toml
[timeperiod.holiday-oncall]
schedule = [{ from = "2026-12-24 00:00", to = "2026-12-27 00:00" }]
```

## Behaviour

How a notify rule with `period` behaves:

- **Inside the time period**, it works like any other rule.
- **Outside it**, its contacts get nothing.
- **When the time period begins**, its contacts get the problems that are
  still open and that they didn't get yet. Problems that recovered or
  were acknowledged in the meantime are not sent. A problem in a downtime
  is sent once the downtime is over, if the time period is still active.
- **After a restart without a database**, problems that were already open
  are not sent when the time period begins, only through `repeat`. With a
  database, Watcher knows from the
  [notification log](../guides/notifications.md#notification-log) who got
  them, and nothing changes.
- **Recoveries** go to the rule's contacts only if they got the problem,
  at any time. So someone who got a problem at 17:55 also gets its
  recovery at 19:00.
- **Repeats** (`repeat`) and **escalations** (`after`) of the rule happen
  only inside the time period. A repeat that falls due outside it is sent
  when the time period begins again.
- Flapping and downtime messages are sent only inside the time period.

A time period that no rule uses gives a warning.

## See also

- [notify](notify.md): the `period` field
- [Alerting recipes](../guides/alerting-recipes.md): on-call at night, the
  team during business hours
