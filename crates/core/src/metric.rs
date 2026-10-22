use crate::threshold::Threshold;
use crate::unit::Unit;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write;

/// A single named measurement produced by a check.
///
/// `value` is always stored in the normalized unit (seconds, bytes, ...), so
/// that `warn`/`crit` and the stored series stay consistent regardless of the
/// unit the plugin printed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Metric {
    pub name: String,
    pub value: f64,
    #[serde(default, skip_serializing_if = "Unit::is_none")]
    pub unit: Unit,
    /// Label set (Prometheus-style), e.g. `{sensor="cpu0"}`. Ordered for
    /// stable series identity.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    /// Thresholds embedded in the output (perfdata or JSON), normalized like
    /// `value`. After evaluation these are the thresholds that were applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warn: Option<Threshold>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crit: Option<Threshold>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
}

impl Metric {
    /// A plain metric with just a name and value, no unit or labels.
    pub fn new(name: impl Into<String>, value: f64) -> Self {
        Metric {
            name: name.into(),
            value,
            unit: Unit::None,
            labels: BTreeMap::new(),
            warn: None,
            crit: None,
            min: None,
            max: None,
        }
    }

    pub fn with_unit(mut self, unit: Unit) -> Self {
        self.unit = unit;
        self
    }

    /// Name and labels in Prometheus notation, e.g. `temp{sensor="cpu0"}`.
    pub fn series_name(&self) -> String {
        let mut out = self.name.clone();
        if !self.labels.is_empty() {
            out.push('{');
            for (i, (k, v)) in self.labels.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                let v = v.replace('\\', "\\\\").replace('"', "\\\"");
                let _ = write!(out, "{k}=\"{v}\"");
            }
            out.push('}');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn series_name_quotes_labels() {
        let mut m = Metric::new("temp", 41.0);
        assert_eq!(m.series_name(), "temp");
        m.labels.insert("sensor".into(), "cpu0".into());
        m.labels.insert("zone".into(), "a\"b".into());
        assert_eq!(m.series_name(), r#"temp{sensor="cpu0",zone="a\"b"}"#);
    }
}
