//! Durations and timestamps as people write and read them: `1h 30m`,
//! `2026-11-19T00:35:09Z`.
//!
//! Timestamps use jiff. Durations have their own small parser: jiff's
//! fixed-length durations reject days and longer, but configs and the API
//! have always taken `1d`, `2w` or `1M`. The syntax is the one humantime
//! accepted, which this module replaces, so existing configs keep working.

use std::fmt;
use std::time::{Duration, SystemTime};

use jiff::Timestamp;
use jiff::civil::DateTime;
use jiff::tz::TimeZone;

/// Why a duration didn't parse. Positions are byte offsets into the text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DurationError {
    #[error("value was empty")]
    Empty,
    #[error("expected number at {0}")]
    NumberExpected(usize),
    #[error("invalid character at {0}")]
    InvalidCharacter(usize),
    #[error("time unit needed, for example {0}sec or {0}ms")]
    UnitNeeded(String),
    #[error(
        "unknown time unit {0:?}, supported units: ns, us/µs, ms, sec, min, hours, days, weeks, months, years (and few variations)"
    )]
    UnknownUnit(String),
    #[error("number is too large")]
    TooLarge,
}

const NANOS_PER_SEC: u128 = 1_000_000_000;
/// A month is 30.44 days and a year 365.25 days, as humantime counted them.
const MONTH_SECS: u64 = 2_630_016;
const YEAR_SECS: u64 = 31_557_600;

/// Nanoseconds per unit; `M` is months, `m` minutes.
fn unit_nanos(unit: &str) -> Option<u128> {
    let secs = |s: u64| Some(u128::from(s) * NANOS_PER_SEC);
    match unit {
        "nanos" | "nsec" | "ns" => Some(1),
        "usec" | "us" | "µs" => Some(1_000),
        "millis" | "msec" | "ms" => Some(1_000_000),
        "seconds" | "second" | "secs" | "sec" | "s" => secs(1),
        "minutes" | "minute" | "min" | "mins" | "m" => secs(60),
        "hours" | "hour" | "hr" | "hrs" | "h" => secs(3600),
        "days" | "day" | "d" => secs(86_400),
        "weeks" | "week" | "wk" | "wks" | "w" => secs(7 * 86_400),
        "months" | "month" | "M" => secs(MONTH_SECS),
        "years" | "year" | "yr" | "yrs" | "y" => secs(YEAR_SECS),
        _ => None,
    }
}

/// Parses `1h 30m`, `1h30m`, `90s`, `1.5h`, `2 days`: numbers with a unit
/// each, added up, with optional spaces in between. A bare `0` needs no
/// unit: configs use it to turn things off (`repeat = "0"`).
pub fn parse_duration(text: &str) -> Result<Duration, DurationError> {
    // Offsets in errors count from the start of `text`.
    let at = |rest: &str| text.len() - rest.len();
    let mut rest = text.trim_start();
    if rest.is_empty() {
        return Err(DurationError::Empty);
    }
    if rest.trim_end() == "0" {
        return Ok(Duration::ZERO);
    }
    let digits = |s: &str| s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let mut total: u128 = 0;
    while !rest.is_empty() {
        let n = digits(rest);
        if n == 0 {
            return Err(DurationError::NumberExpected(at(rest)));
        }
        let (whole, after) = rest.split_at(n);
        let whole: u128 = whole.parse().map_err(|_| DurationError::TooLarge)?;
        rest = after;

        let mut fraction = "";
        if let Some(after) = rest.strip_prefix('.') {
            let n = digits(after);
            if n == 0 {
                return Err(DurationError::InvalidCharacter(at(rest)));
            }
            fraction = &after[..n];
            rest = &after[n..];
        }

        rest = rest.trim_start();
        let n = rest
            .find(|c: char| !(c.is_ascii_alphabetic() || c == 'µ'))
            .unwrap_or(rest.len());
        let (unit, after) = rest.split_at(n);
        if unit.is_empty() {
            return match rest.chars().next() {
                None => Err(DurationError::UnitNeeded(whole.to_string())),
                Some(_) => Err(DurationError::InvalidCharacter(at(rest))),
            };
        }
        let per = unit_nanos(unit).ok_or_else(|| DurationError::UnknownUnit(unit.into()))?;
        rest = after.trim_start();

        // Digits past nanoseconds of a year don't change anything.
        let fraction = &fraction[..fraction.len().min(18)];
        let part = if fraction.is_empty() {
            0
        } else {
            let scale = 10u128.pow(fraction.len() as u32);
            let numerator: u128 = fraction.parse().map_err(|_| DurationError::TooLarge)?;
            numerator * per / scale
        };
        total = whole
            .checked_mul(per)
            .and_then(|t| t.checked_add(part))
            .and_then(|t| t.checked_add(total))
            .ok_or(DurationError::TooLarge)?;
    }
    let secs = u64::try_from(total / NANOS_PER_SEC).map_err(|_| DurationError::TooLarge)?;
    Ok(Duration::new(secs, (total % NANOS_PER_SEC) as u32))
}

/// `1h 30m`, `2days 4h`, `0s`: the largest units first, zeros left out.
pub fn format_duration(d: Duration) -> FormattedDuration {
    FormattedDuration(d)
}

/// A duration for display; see [`format_duration`].
#[derive(Debug, Clone, Copy)]
pub struct FormattedDuration(Duration);

impl fmt::Display for FormattedDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let secs = self.0.as_secs();
        let nanos = u64::from(self.0.subsec_nanos());
        if secs == 0 && nanos == 0 {
            return f.write_str("0s");
        }
        let in_year = secs % YEAR_SECS;
        let in_month = in_year % MONTH_SECS;
        let in_day = in_month % 86_400;
        // Calendar-sized units are words with a plural; the others symbols.
        let items: [(u64, &str, bool); 9] = [
            (secs / YEAR_SECS, "year", true),
            (in_year / MONTH_SECS, "month", true),
            (in_month / 86_400, "day", true),
            (in_day / 3600, "h", false),
            (in_day % 3600 / 60, "m", false),
            (in_day % 60, "s", false),
            (nanos / 1_000_000, "ms", false),
            (nanos / 1000 % 1000, "us", false),
            (nanos % 1000, "ns", false),
        ];
        let mut first = true;
        for (value, name, plural) in items {
            if value == 0 {
                continue;
            }
            if !first {
                f.write_str(" ")?;
            }
            first = false;
            write!(f, "{value}{name}")?;
            if plural && value > 1 {
                f.write_str("s")?;
            }
        }
        Ok(())
    }
}

/// A time in UTC as RFC 3339, with a fixed number of decimals (cut off,
/// not rounded): `2026-11-19T00:35:09Z`, `2026-11-19T00:35:09.120Z`.
#[derive(Debug, Clone, Copy)]
pub struct Rfc3339 {
    at: Timestamp,
    decimals: usize,
}

impl fmt::Display for Rfc3339 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.*}", self.decimals, self.at)
    }
}

fn timestamp(t: SystemTime) -> Timestamp {
    // Out of jiff's range (years -9999 to 9999): the nearest end.
    Timestamp::try_from(t).unwrap_or(if t > SystemTime::UNIX_EPOCH {
        Timestamp::MAX
    } else {
        Timestamp::MIN
    })
}

/// `2026-11-19T00:35:09Z`.
pub fn rfc3339_seconds(t: SystemTime) -> Rfc3339 {
    Rfc3339 {
        at: timestamp(t),
        decimals: 0,
    }
}

/// `2026-11-19T00:35:09.120Z`.
pub fn rfc3339_millis(t: SystemTime) -> Rfc3339 {
    Rfc3339 {
        at: timestamp(t),
        decimals: 3,
    }
}

/// A time as people type it: RFC 3339, or without the offset (then UTC),
/// with a space for the `T`, with or without decimals:
/// `2026-11-19T00:35:09Z`, `2026-11-19 00:35:09`, `2026-11-19 00:35:09.5`.
pub fn parse_rfc3339_weak(text: &str) -> Result<SystemTime, jiff::Error> {
    let text = text.trim();
    let at = match text.parse::<Timestamp>() {
        Ok(at) => at,
        Err(_) => text
            .parse::<DateTime>()?
            .to_zoned(TimeZone::UTC)?
            .timestamp(),
    };
    Ok(at.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    #[test]
    fn parses_what_configs_use() {
        let cases = [
            ("30s", secs(30)),
            ("5m", secs(300)),
            ("1h", secs(3600)),
            ("1h30m", secs(5400)),
            ("1h 30m", secs(5400)),
            ("  2 hours  ", secs(7200)),
            ("1d", secs(86_400)),
            ("7d", secs(7 * 86_400)),
            ("2w", secs(14 * 86_400)),
            ("1M", secs(MONTH_SECS)),
            ("1y", secs(YEAR_SECS)),
            ("1.5h", secs(5400)),
            ("1h 1.5m", secs(3690)),
            ("500ms", Duration::from_millis(500)),
            ("1s 250ms", Duration::from_millis(1250)),
            ("3us", Duration::from_micros(3)),
            ("3µs", Duration::from_micros(3)),
            ("7ns", Duration::from_nanos(7)),
            ("2min 5secs", secs(125)),
            ("0s", secs(0)),
            ("0", secs(0)),
        ];
        for (text, want) in cases {
            assert_eq!(parse_duration(text), Ok(want), "{text:?}");
        }
    }

    #[test]
    fn rejects_what_is_not_a_duration() {
        assert_eq!(parse_duration(""), Err(DurationError::Empty));
        assert_eq!(parse_duration("  "), Err(DurationError::Empty));
        assert_eq!(parse_duration("h"), Err(DurationError::NumberExpected(0)));
        assert_eq!(
            parse_duration("10"),
            Err(DurationError::UnitNeeded("10".into()))
        );
        assert_eq!(
            parse_duration("5 parsecs"),
            Err(DurationError::UnknownUnit("parsecs".into()))
        );
        assert_eq!(
            parse_duration("1h, 5m"),
            Err(DurationError::NumberExpected(2))
        );
        assert_eq!(parse_duration("5m!"), Err(DurationError::NumberExpected(2)));
        assert_eq!(
            parse_duration("1.h"),
            Err(DurationError::InvalidCharacter(1))
        );
        assert_eq!(parse_duration("-5m"), Err(DurationError::NumberExpected(0)));
        assert_eq!(
            parse_duration("99999999999999999999999y"),
            Err(DurationError::TooLarge)
        );
        assert_eq!(
            parse_duration("10").unwrap_err().to_string(),
            "time unit needed, for example 10sec or 10ms"
        );
    }

    #[test]
    fn formats_largest_units_first() {
        let cases = [
            (secs(0), "0s"),
            (secs(5), "5s"),
            (secs(300), "5m"),
            (secs(5400), "1h 30m"),
            (secs(86_400 + 7200), "1day 2h"),
            (secs(3 * 86_400), "3days"),
            (secs(YEAR_SECS + 2 * MONTH_SECS), "1year 2months"),
            (Duration::from_millis(1250), "1s 250ms"),
            (Duration::from_nanos(1_002_003), "1ms 2us 3ns"),
        ];
        for (d, want) in cases {
            assert_eq!(format_duration(d).to_string(), want);
        }
    }

    #[test]
    fn formatted_durations_parse_back() {
        for s in [1, 59, 61, 3599, 3661, 86_399, 90_061, 1_000_000] {
            let d = secs(s);
            assert_eq!(parse_duration(&format_duration(d).to_string()), Ok(d));
        }
    }

    #[test]
    fn timestamps_cut_off_decimals() {
        let t = SystemTime::UNIX_EPOCH + Duration::new(1_795_000_000, 999_999_999);
        assert_eq!(rfc3339_seconds(t).to_string(), "2026-11-18T11:06:40Z");
        assert_eq!(rfc3339_millis(t).to_string(), "2026-11-18T11:06:40.999Z");
        let t = SystemTime::UNIX_EPOCH + secs(5);
        assert_eq!(rfc3339_millis(t).to_string(), "1970-01-01T00:00:05.000Z");
    }

    #[test]
    fn weak_timestamps_are_utc() {
        let want = SystemTime::UNIX_EPOCH + secs(1_795_000_000);
        for text in [
            "2026-11-18T11:06:40Z",
            "2026-11-18T11:06:40+00:00",
            "2026-11-18T12:06:40+01:00",
            "2026-11-18T11:06:40",
            "2026-11-18 11:06:40",
            " 2026-11-18 11:06:40 ",
        ] {
            assert_eq!(parse_rfc3339_weak(text).ok(), Some(want), "{text:?}");
        }
        assert_eq!(
            parse_rfc3339_weak("2026-11-18 11:06:40.5").ok(),
            Some(want + Duration::from_millis(500))
        );
        assert!(parse_rfc3339_weak("6h").is_err());
        assert!(parse_rfc3339_weak("yesterday").is_err());
    }
}
