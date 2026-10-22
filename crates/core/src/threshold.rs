use crate::num::split_number;
use crate::unit::{Unit, format_value};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// Comparison operator for the simple threshold syntax.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Gt,
    Ge,
    Lt,
    Le,
    Eq,
    Ne,
}

impl CmpOp {
    pub fn symbol(self) -> &'static str {
        match self {
            CmpOp::Gt => ">",
            CmpOp::Ge => ">=",
            CmpOp::Lt => "<",
            CmpOp::Le => "<=",
            CmpOp::Eq => "==",
            CmpOp::Ne => "!=",
        }
    }
}

/// A parsed threshold.
///
/// Two syntaxes are supported:
/// * comparisons — `> 100`, `>= 10GB`, `< 5`, `== 0`, `!= 0`
/// * Nagios ranges — `10`, `10:`, `~:10`, `10:20`, `@10:20`
///
/// In both cases [`Threshold::breached`] returns `true` when the value should
/// raise an alert.
#[derive(Debug, Clone, PartialEq)]
pub enum Threshold {
    Cmp {
        op: CmpOp,
        bound: f64,
    },
    /// Nagios range: alert when the value is outside `[lo, hi]`, or inside it
    /// when `inside` is set (the `@` prefix).
    Range {
        lo: f64,
        hi: f64,
        inside: bool,
    },
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ThresholdError {
    #[error("empty threshold")]
    Empty,
    #[error("invalid threshold {0:?}")]
    Invalid(String),
}

impl Threshold {
    /// Parses a threshold. Any unit suffix on a bound (`500ms`, `10GB`) is
    /// normalized the same way metric values are, so comparisons happen in the
    /// stored unit.
    ///
    /// Bounds without a unit are taken as already normalized.
    pub fn parse(spec: &str) -> Result<Self, ThresholdError> {
        Self::parse_scaled(spec, 1.0)
    }

    /// Like [`Threshold::parse`], but bounds without a unit are multiplied by
    /// `factor`. Used for thresholds embedded in perfdata, which are given in
    /// the metric's own unit (`5948` next to a value in `MB`).
    pub(crate) fn parse_scaled(spec: &str, factor: f64) -> Result<Self, ThresholdError> {
        let s = spec.trim();
        if s.is_empty() {
            return Err(ThresholdError::Empty);
        }

        if let Some(op) = s
            .strip_prefix(">=")
            .map(|r| (CmpOp::Ge, r))
            .or_else(|| s.strip_prefix("<=").map(|r| (CmpOp::Le, r)))
            .or_else(|| s.strip_prefix("==").map(|r| (CmpOp::Eq, r)))
            .or_else(|| s.strip_prefix("!=").map(|r| (CmpOp::Ne, r)))
            .or_else(|| s.strip_prefix('>').map(|r| (CmpOp::Gt, r)))
            .or_else(|| s.strip_prefix('<').map(|r| (CmpOp::Lt, r)))
        {
            let (op, rest) = op;
            let bound =
                parse_bound(rest, factor).ok_or_else(|| ThresholdError::Invalid(spec.into()))?;
            return Ok(Threshold::Cmp { op, bound });
        }

        Self::parse_range(s, factor).ok_or_else(|| ThresholdError::Invalid(spec.into()))
    }

    fn parse_range(spec: &str, factor: f64) -> Option<Self> {
        let (inside, body) = match spec.strip_prefix('@') {
            Some(rest) => (true, rest),
            None => (false, spec),
        };

        let (lo, hi) = match body.split_once(':') {
            // `lo:hi`, with `lo` or `hi` possibly empty or `~`.
            Some((lo_s, hi_s)) => {
                let lo = match lo_s.trim() {
                    "~" => f64::NEG_INFINITY,
                    "" => 0.0,
                    other => parse_bound(other, factor)?,
                };
                let hi = match hi_s.trim() {
                    "" => f64::INFINITY,
                    other => parse_bound(other, factor)?,
                };
                (lo, hi)
            }
            // Bare `hi` means `0:hi`.
            None => (0.0, parse_bound(body, factor)?),
        };

        if lo > hi {
            return None;
        }
        Some(Threshold::Range { lo, hi, inside })
    }

    /// Returns `true` when `value` (in its normalized unit) breaches the
    /// threshold and should raise an alert.
    pub fn breached(&self, value: f64) -> bool {
        match *self {
            Threshold::Cmp { op, bound } => match op {
                CmpOp::Gt => value > bound,
                CmpOp::Ge => value >= bound,
                CmpOp::Lt => value < bound,
                CmpOp::Le => value <= bound,
                CmpOp::Eq => value == bound,
                CmpOp::Ne => value != bound,
            },
            Threshold::Range { lo, hi, inside } => {
                let outside = value < lo || value > hi;
                if inside { !outside } else { outside }
            }
        }
    }
}

impl Threshold {
    /// Human-readable form with bounds formatted in `unit`, e.g. `> 500 ms`
    /// or `0:5.8 GiB`. Unlike [`Display`](fmt::Display), this does not parse back.
    pub fn describe(&self, unit: &Unit) -> String {
        let bound = |v: f64| {
            if v == 0.0 {
                "0".to_string()
            } else {
                format_value(v, unit)
            }
        };
        match *self {
            Threshold::Cmp { op, bound: b } => format!("{} {}", op.symbol(), bound(b)),
            Threshold::Range { lo, hi, inside } => {
                let lo = if lo.is_infinite() {
                    "~".to_string()
                } else {
                    bound(lo)
                };
                let hi = if hi.is_finite() {
                    bound(hi)
                } else {
                    String::new()
                };
                format!("{}{lo}:{hi}", if inside { "@" } else { "" })
            }
        }
    }
}

/// A threshold as written in the config.
///
/// Bounds without a unit are read relative to the metric the threshold is
/// applied to: milliseconds for time metrics, the normalized unit (bytes,
/// percent, ...) for everything else. It is therefore kept as validated text
/// and resolved per metric with [`ConfigThreshold::resolve`].
///
/// `_` may group digits (`> 10_000`). `,` and `.` never group, since plugin
/// output uses `,` as a decimal comma.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigThreshold {
    /// As written, for display and write-back.
    text: String,
    /// With digit grouping removed, ready for [`Threshold::parse_scaled`].
    spec: String,
}

impl ConfigThreshold {
    /// Factors for bare bounds: milliseconds for time, 1 for everything else.
    const FACTORS: [f64; 2] = [1e-3, 1.0];

    /// Validates the threshold for every unit it may be resolved against, so
    /// that `10:2s` (fine in ms, but 10 > 2 otherwise) is rejected up front.
    pub fn parse(text: &str) -> Result<Self, ThresholdError> {
        let text = text.trim();
        let spec = ungroup_digits(text);
        // A `_` left next to a digit is misplaced grouping (`10_`, `_10`, `1__0`).
        let digit_next_to_underscore = spec.as_bytes().windows(2).any(|w| {
            (w[0] == b'_' && w[1].is_ascii_digit()) || (w[0].is_ascii_digit() && w[1] == b'_')
        });
        if digit_next_to_underscore {
            return Err(ThresholdError::Invalid(text.into()));
        }
        for factor in Self::FACTORS {
            Threshold::parse_scaled(&spec, factor)
                .map_err(|_| ThresholdError::Invalid(text.into()))?;
        }
        Ok(ConfigThreshold {
            text: text.to_string(),
            spec,
        })
    }

    /// The threshold for a metric in `unit`.
    pub fn resolve(&self, unit: &Unit) -> Threshold {
        let factor = match unit {
            Unit::Seconds => Self::FACTORS[0],
            _ => Self::FACTORS[1],
        };
        Threshold::parse_scaled(&self.spec, factor).expect("validated in ConfigThreshold::parse")
    }
}

impl fmt::Display for ConfigThreshold {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl Serialize for ConfigThreshold {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.text)
    }
}

/// Accepts a string or, for a bare Nagios range like `warn = 10_000`, a number.
impl<'de> Deserialize<'de> for ConfigThreshold {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Text(String),
            Int(i64),
            Float(f64),
        }
        let text = match Raw::deserialize(deserializer)? {
            Raw::Text(s) => s,
            Raw::Int(n) => n.to_string(),
            Raw::Float(n) => n.to_string(),
        };
        ConfigThreshold::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// Removes `_` between two digits.
fn ungroup_digits(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    chars
        .iter()
        .enumerate()
        .filter(|&(i, &c)| {
            let grouping = c == '_'
                && i > 0
                && chars[i - 1].is_ascii_digit()
                && chars.get(i + 1).is_some_and(char::is_ascii_digit);
            !grouping
        })
        .map(|(_, &c)| c)
        .collect()
}

/// Parses a numeric bound with an optional unit suffix, normalizing it.
/// `default_factor` applies when no unit is given.
fn parse_bound(s: &str, default_factor: f64) -> Option<f64> {
    let (value, rest) = split_number(s.trim())?;
    let factor = match rest.trim() {
        "" => default_factor,
        unit => Unit::parse(unit).1,
    };
    Some(value * factor)
}

impl fmt::Display for Threshold {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Threshold::Cmp { op, bound } => write!(f, "{} {bound}", op.symbol()),
            // Written in Nagios syntax so that the text parses back.
            Threshold::Range { lo, hi, inside } => {
                if *inside {
                    write!(f, "@")?;
                }
                if lo.is_infinite() {
                    write!(f, "~")?;
                } else {
                    write!(f, "{lo}")?;
                }
                write!(f, ":")?;
                if hi.is_finite() {
                    write!(f, "{hi}")?;
                }
                Ok(())
            }
        }
    }
}

impl Serialize for Threshold {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Threshold {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let spec = String::deserialize(deserializer)?;
        Threshold::parse(&spec).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparison_syntax() {
        assert!(Threshold::parse("> 100").unwrap().breached(101.0));
        assert!(!Threshold::parse("> 100").unwrap().breached(100.0));
        assert!(Threshold::parse(">= 100").unwrap().breached(100.0));
        assert!(Threshold::parse("< 5").unwrap().breached(4.0));
        assert!(Threshold::parse("== 0").unwrap().breached(0.0));
        assert!(Threshold::parse("!= 0").unwrap().breached(1.0));
    }

    #[test]
    fn comparison_bounds_are_unit_normalized() {
        // 500ms -> 0.5 s, so a 0.6 s value breaches.
        let t = Threshold::parse("> 500ms").unwrap();
        assert!(t.breached(0.6));
        assert!(!t.breached(0.4));
        // 1GB -> 2^30 bytes.
        let t = Threshold::parse(">= 1GB").unwrap();
        assert!(t.breached(1024.0 * 1024.0 * 1024.0));
    }

    #[test]
    fn nagios_ranges() {
        // bare number: alert outside 0..=10
        let t = Threshold::parse("10").unwrap();
        assert!(t.breached(-1.0));
        assert!(t.breached(11.0));
        assert!(!t.breached(5.0));

        // 10: alert when below 10
        let t = Threshold::parse("10:").unwrap();
        assert!(t.breached(9.0));
        assert!(!t.breached(10.0));

        // ~:10 alert when above 10
        let t = Threshold::parse("~:10").unwrap();
        assert!(t.breached(11.0));
        assert!(!t.breached(-100.0));

        // 10:20 alert when outside
        let t = Threshold::parse("10:20").unwrap();
        assert!(t.breached(9.0));
        assert!(t.breached(21.0));
        assert!(!t.breached(15.0));

        // @10:20 alert when inside
        let t = Threshold::parse("@10:20").unwrap();
        assert!(t.breached(15.0));
        assert!(!t.breached(9.0));
    }

    #[test]
    fn scaled_bounds_use_the_metric_unit_unless_given() {
        let mb = 1024.0 * 1024.0;
        let t = Threshold::parse_scaled("5948", mb).unwrap();
        assert_eq!(
            t,
            Threshold::Range {
                lo: 0.0,
                hi: 5948.0 * mb,
                inside: false
            }
        );
        // An explicit unit wins over the factor.
        let t = Threshold::parse_scaled("> 1KB", mb).unwrap();
        assert_eq!(
            t,
            Threshold::Cmp {
                op: CmpOp::Gt,
                bound: 1024.0
            }
        );
    }

    #[test]
    fn display_round_trips() {
        for spec in ["> 100", "<= 0.5", "0:10", "10:", "~:10", "@10:20", "~:"] {
            let t = Threshold::parse(spec).unwrap();
            assert_eq!(Threshold::parse(&t.to_string()).unwrap(), t, "{spec}");
        }
        assert_eq!(Threshold::parse("~:10").unwrap().to_string(), "~:10");
        assert_eq!(Threshold::parse("10:").unwrap().to_string(), "10:");
    }

    #[test]
    fn describe_uses_the_unit() {
        let t = Threshold::parse("> 500ms").unwrap();
        assert_eq!(t.describe(&Unit::Seconds), "> 500 ms");
        let t = Threshold::parse("@~:5GB").unwrap();
        assert_eq!(t.describe(&Unit::Bytes), "@~:5 GiB");
        let t = Threshold::parse("80").unwrap();
        assert_eq!(t.describe(&Unit::Percent), "0:80%");
    }

    #[test]
    fn config_thresholds_default_to_ms_for_time() {
        let t = ConfigThreshold::parse("> 500").unwrap();
        assert_eq!(
            t.resolve(&Unit::Seconds),
            Threshold::parse("> 0.5").unwrap()
        );
        assert_eq!(t.resolve(&Unit::Bytes), Threshold::parse("> 500").unwrap());
        // An explicit unit is not affected.
        let t = ConfigThreshold::parse("1:2s").unwrap();
        assert_eq!(
            t.resolve(&Unit::Seconds),
            Threshold::parse("0.001:2").unwrap()
        );
        assert!(ConfigThreshold::parse("20:10").is_err());
        // Valid in ms, but 10 > 2 for any other unit.
        assert!(ConfigThreshold::parse("10:2s").is_err());
    }

    #[test]
    fn config_thresholds_allow_digit_grouping() {
        let t = ConfigThreshold::parse("> 10_000").unwrap();
        assert_eq!(t.resolve(&Unit::None), Threshold::parse("> 10000").unwrap());
        assert_eq!(t.to_string(), "> 10_000");
        let t = ConfigThreshold::parse("@1_000:1_000_000KB").unwrap();
        assert_eq!(
            t.resolve(&Unit::Bytes),
            Threshold::parse("@1000:1000000KB").unwrap()
        );
        for bad in ["> _10", "> 10_", "> 10__000", "~:_5"] {
            assert_eq!(
                ConfigThreshold::parse(bad),
                Err(ThresholdError::Invalid(bad.into())),
                "{bad}"
            );
        }
        // `,` stays a decimal comma.
        let t = ConfigThreshold::parse("> 10,5").unwrap();
        assert_eq!(t.resolve(&Unit::None), Threshold::parse("> 10.5").unwrap());
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(Threshold::parse(""), Err(ThresholdError::Empty));
        assert!(Threshold::parse("> abc").is_err());
        assert!(Threshold::parse("20:10").is_err()); // lo > hi
    }
}
