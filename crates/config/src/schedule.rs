//! Downtime windows of downtime groups, in local time: weekly
//! (`{ days = ["sun"], from = "02:00", to = "04:00" }`) or once
//! (`{ from = "2026-11-01 22:00", to = "2026-11-02 02:00" }`).

use jiff::civil::{Date, DateTime, Time, Weekday};
use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan};
use std::fmt;
use std::time::SystemTime;
use toml::{Table, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Window {
    /// On `days` (every day when empty) from `from` to `to`; a `to` not
    /// after `from` ends the next day.
    Weekly {
        days: Vec<Weekday>,
        from: Time,
        to: Time,
    },
    Once {
        from: DateTime,
        to: DateTime,
    },
}

const DAYS: [(&str, Weekday); 7] = [
    ("mon", Weekday::Monday),
    ("tue", Weekday::Tuesday),
    ("wed", Weekday::Wednesday),
    ("thu", Weekday::Thursday),
    ("fri", Weekday::Friday),
    ("sat", Weekday::Saturday),
    ("sun", Weekday::Sunday),
];

impl Window {
    /// Parses one entry of `schedule`.
    pub fn parse(value: &Value) -> Result<Self, String> {
        let Value::Table(t) = value else {
            return Err(
                "expected a table like { days = [\"sun\"], from = \"02:00\", to = \"04:00\" }"
                    .into(),
            );
        };
        for key in t.keys() {
            if !["days", "from", "to"].contains(&key.as_str()) {
                return Err(format!("unknown field {key:?} (expected days, from, to)"));
            }
        }
        let text = |t: &Table, key: &str| -> Result<String, String> {
            match t.get(key) {
                Some(Value::String(s)) => Ok(s.trim().to_string()),
                Some(_) => Err(format!("{key}: expected a string")),
                None => Err(format!("needs {key}")),
            }
        };
        let (from, to) = (text(t, "from")?, text(t, "to")?);
        if from.contains('-') || to.contains('-') {
            if t.contains_key("days") {
                return Err("days: not with dates in from/to".into());
            }
            let from = datetime(&from)?;
            let to = datetime(&to)?;
            if to <= from {
                return Err("to must be after from".into());
            }
            return Ok(Window::Once { from, to });
        }
        let days = match t.get("days") {
            None => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .map(|v| match v {
                    Value::String(s) => day(s),
                    _ => Err("days: expected strings".into()),
                })
                .collect::<Result<_, _>>()?,
            Some(Value::String(s)) => vec![day(s)?],
            Some(_) => return Err("days: expected a list like [\"sat\", \"sun\"]".into()),
        };
        Ok(Window::Weekly {
            days,
            from: time(&from)?,
            to: time(&to)?,
        })
    }

    /// The occurrence that contains `now`, as `(start, end)`.
    pub fn active(&self, now: SystemTime, tz: &TimeZone) -> Option<(SystemTime, SystemTime)> {
        let now = Timestamp::try_from(now).ok()?;
        let today = now.to_zoned(tz.clone()).date();
        self.occurrences(today.checked_sub(1.day()).ok()?, 2, tz)
            .into_iter()
            .find(|(start, end)| *start <= now && now < *end)
            .map(|(s, e)| (s.into(), e.into()))
    }

    /// The next occurrence starting after `now`, within a week.
    pub fn next(&self, now: SystemTime, tz: &TimeZone) -> Option<(SystemTime, SystemTime)> {
        let now = Timestamp::try_from(now).ok()?;
        let today = now.to_zoned(tz.clone()).date();
        self.occurrences(today, 8, tz)
            .into_iter()
            .find(|(start, _)| *start > now)
            .map(|(s, e)| (s.into(), e.into()))
    }

    /// Occurrences starting on `days` days from `first`.
    fn occurrences(&self, first: Date, days: i64, tz: &TimeZone) -> Vec<(Timestamp, Timestamp)> {
        let at = |dt: DateTime| {
            tz.to_ambiguous_zoned(dt)
                .compatible()
                .ok()
                .map(|z| z.timestamp())
        };
        match self {
            Window::Once { from, to } => at(*from).zip(at(*to)).into_iter().collect(),
            Window::Weekly { days: on, from, to } => (0..days)
                .filter_map(|n| first.checked_add(n.days()).ok())
                .filter(|d| on.is_empty() || on.contains(&d.weekday()))
                .filter_map(|d| {
                    let end_day = if to > from {
                        d
                    } else {
                        d.checked_add(1.day()).ok()?
                    };
                    Some((at(d.to_datetime(*from))?, at(end_day.to_datetime(*to))?))
                })
                .collect(),
        }
    }
}

impl fmt::Display for Window {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hm = |t: &Time| format!("{:02}:{:02}", t.hour(), t.minute());
        match self {
            Window::Weekly { days, from, to } => {
                let names: Vec<&str> = days
                    .iter()
                    .filter_map(|d| DAYS.iter().find(|(_, w)| w == d).map(|(n, _)| *n))
                    .collect();
                let on = match names.is_empty() {
                    true => "daily".to_string(),
                    false => names.join(", "),
                };
                write!(f, "{on} {}-{}", hm(from), hm(to))
            }
            Window::Once { from, to } => write!(
                f,
                "{} {} - {} {}",
                from.date(),
                hm(&from.time()),
                to.date(),
                hm(&to.time())
            ),
        }
    }
}

fn day(s: &str) -> Result<Weekday, String> {
    let s = s.trim().to_lowercase();
    DAYS.iter()
        .find(|(name, _)| s.starts_with(name) && s.len() >= 3)
        .map(|(_, d)| *d)
        .ok_or_else(|| format!("unknown day {s:?} (mon, tue, wed, thu, fri, sat, sun)"))
}

fn time(s: &str) -> Result<Time, String> {
    let (h, m) = s
        .split_once(':')
        .ok_or_else(|| format!("invalid time {s:?} (expected HH:MM)"))?;
    let (h, m): (i8, i8) = (
        h.parse().map_err(|_| format!("invalid time {s:?}"))?,
        m.parse().map_err(|_| format!("invalid time {s:?}"))?,
    );
    // 24:00 is the end of the day.
    if (h, m) == (24, 0) {
        return Ok(Time::MIN);
    }
    Time::new(h, m, 0, 0).map_err(|_| format!("invalid time {s:?}"))
}

fn datetime(s: &str) -> Result<DateTime, String> {
    let (date, t) = s
        .split_once([' ', 'T'])
        .ok_or_else(|| format!("invalid date {s:?} (expected YYYY-MM-DD HH:MM)"))?;
    let date: Date = date
        .parse()
        .map_err(|_| format!("invalid date {s:?} (expected YYYY-MM-DD HH:MM)"))?;
    Ok(date.to_datetime(time(t)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(toml: &str) -> Window {
        let v: Table = toml::from_str(&format!("w = {toml}")).unwrap();
        Window::parse(&v["w"]).unwrap()
    }

    fn at(s: &str) -> SystemTime {
        let dt: DateTime = s.parse().unwrap();
        dt.to_zoned(TimeZone::UTC).unwrap().timestamp().into()
    }

    #[test]
    fn weekly_windows() {
        let utc = TimeZone::UTC;
        // 2026-10-25 is a Sunday.
        let w = window(r#"{ days = ["sun"], from = "02:00", to = "04:00" }"#);
        assert_eq!(w.to_string(), "sun 02:00-04:00");
        assert_eq!(
            w.active(at("2026-10-25T03:00"), &utc),
            Some((at("2026-10-25T02:00"), at("2026-10-25T04:00")))
        );
        assert_eq!(w.active(at("2026-10-25T04:00"), &utc), None);
        assert_eq!(w.active(at("2026-10-24T03:00"), &utc), None);
        assert_eq!(
            w.next(at("2026-10-23T12:00"), &utc),
            Some((at("2026-10-25T02:00"), at("2026-10-25T04:00")))
        );

        // Across midnight: Saturday 22:00 to Sunday 02:00.
        let w = window(r#"{ days = ["saturday"], from = "22:00", to = "02:00" }"#);
        assert!(w.active(at("2026-10-25T01:00"), &utc).is_some());
        assert!(w.active(at("2026-10-24T21:00"), &utc).is_none());

        let daily = window(r#"{ from = "12:00", to = "12:30" }"#);
        assert_eq!(daily.to_string(), "daily 12:00-12:30");
        assert!(daily.active(at("2026-10-21T12:10"), &utc).is_some());
    }

    #[test]
    fn once_and_errors() {
        let utc = TimeZone::UTC;
        let w = window(r#"{ from = "2026-11-01 22:00", to = "2026-11-02 02:00" }"#);
        assert!(w.active(at("2026-11-02T01:00"), &utc).is_some());
        assert!(w.active(at("2026-11-02T02:00"), &utc).is_none());
        assert_eq!(w.to_string(), "2026-11-01 22:00 - 2026-11-02 02:00");

        let err = |toml: &str| {
            let v: Table = toml::from_str(&format!("w = {toml}")).unwrap();
            Window::parse(&v["w"]).unwrap_err()
        };
        assert!(err(r#"{ from = "02:00" }"#).contains("needs to"));
        assert!(
            err(r#"{ days = ["sonntag"], from = "02:00", to = "03:00" }"#).contains("unknown day")
        );
        assert!(err(r#"{ from = "25:00", to = "03:00" }"#).contains("invalid time"));
        assert!(err(r#"{ from = "2026-11-02 02:00", to = "2026-11-01 02:00" }"#).contains("after"));
        assert!(err(r#"{ from = "1", to = "2", at = "x" }"#).contains("unknown field"));
    }
}
