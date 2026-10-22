use serde::{Serialize, Serializer};

/// Unit of a metric after normalization.
///
/// Time is stored in seconds and sizes in bytes, so a check switching from
/// `MB` to `B` does not produce a jump in the stored series.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Unit {
    #[default]
    None,
    Seconds,
    Bytes,
    Percent,
    Counter,
    /// Any other unit (`°C`, `rpm`, ...), kept verbatim.
    Other(String),
}

impl Unit {
    /// Parses a unit suffix and returns the normalized unit together with the
    /// factor that converts a value in `s` to that unit.
    ///
    /// `KB`, `MB`, ... follow the Nagios convention (×1024), as do the IEC
    /// forms `KiB`, `MiB`, ...; only the SI spelling `kB` means ×1000.
    pub fn parse(s: &str) -> (Unit, f64) {
        const KI: f64 = 1024.0;
        match s.trim() {
            "" => (Unit::None, 1.0),
            "s" => (Unit::Seconds, 1.0),
            "ms" => (Unit::Seconds, 1e-3),
            "us" | "µs" | "μs" => (Unit::Seconds, 1e-6),
            "ns" => (Unit::Seconds, 1e-9),
            "%" => (Unit::Percent, 1.0),
            "c" => (Unit::Counter, 1.0),
            "B" => (Unit::Bytes, 1.0),
            "kB" => (Unit::Bytes, 1e3),
            "KB" | "KiB" => (Unit::Bytes, KI),
            "MB" | "MiB" => (Unit::Bytes, KI * KI),
            "GB" | "GiB" => (Unit::Bytes, KI * KI * KI),
            "TB" | "TiB" => (Unit::Bytes, KI * KI * KI * KI),
            "PB" | "PiB" => (Unit::Bytes, KI * KI * KI * KI * KI),
            other => (Unit::Other(other.to_string()), 1.0),
        }
    }

    pub fn is_none(&self) -> bool {
        *self == Unit::None
    }

    /// Canonical symbol of the normalized unit (empty for [`Unit::None`]).
    pub fn symbol(&self) -> &str {
        match self {
            Unit::None => "",
            Unit::Seconds => "s",
            Unit::Bytes => "B",
            Unit::Percent => "%",
            Unit::Counter => "c",
            Unit::Other(s) => s,
        }
    }
}

impl Serialize for Unit {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.symbol())
    }
}

/// Formats a normalized value for humans, e.g. `1.5 GiB`, `12 ms` or `56%`.
pub fn format_value(value: f64, unit: &Unit) -> String {
    match unit {
        Unit::None | Unit::Counter => trim_number(value),
        Unit::Percent => format!("{}%", trim_number(value)),
        Unit::Other(u) => format!("{}{u}", trim_number(value)),
        Unit::Seconds => {
            let abs = value.abs();
            if abs != 0.0 && abs < 1e-3 {
                format!("{} µs", trim_number(value * 1e6))
            } else if abs != 0.0 && abs < 1.0 {
                format!("{} ms", trim_number(value * 1e3))
            } else {
                format!("{} s", trim_number(value))
            }
        }
        Unit::Bytes => {
            const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
            let mut scaled = value;
            let mut idx = 0;
            while scaled.abs() >= 1024.0 && idx < UNITS.len() - 1 {
                scaled /= 1024.0;
                idx += 1;
            }
            format!("{} {}", trim_number(scaled), UNITS[idx])
        }
    }
}

/// Formats with up to three decimals and drops trailing zeros.
fn trim_number(value: f64) -> String {
    let s = format!("{value:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_time_and_size() {
        assert_eq!(Unit::parse("ms"), (Unit::Seconds, 1e-3));
        assert_eq!(Unit::parse("MB"), (Unit::Bytes, 1024.0 * 1024.0));
        assert_eq!(Unit::parse("kB"), (Unit::Bytes, 1000.0));
        assert_eq!(Unit::parse("°C"), (Unit::Other("°C".into()), 1.0));
    }

    #[test]
    fn formats_values() {
        assert_eq!(format_value(0.012, &Unit::Seconds), "12 ms");
        assert_eq!(format_value(2.5, &Unit::Seconds), "2.5 s");
        assert_eq!(
            format_value(1536.0 * 1024.0 * 1024.0, &Unit::Bytes),
            "1.5 GiB"
        );
        assert_eq!(format_value(56.0, &Unit::Percent), "56%");
        assert_eq!(format_value(41.25, &Unit::Other("°C".into())), "41.25°C");
        assert_eq!(format_value(-0.0001, &Unit::None), "0");
    }
}
