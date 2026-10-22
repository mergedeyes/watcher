use super::{Format, ParseError, ParsedOutput};
use crate::metric::Metric;
use crate::num::split_number;
use crate::state::State;
use crate::threshold::Threshold;
use crate::unit::Unit;
use serde_json::{Map, Value};

/// Parses JSON output:
///
/// ```json
/// {"state": "warning", "message": "queue is filling",
///  "metrics": [{"name": "depth", "value": 120, "labels": {"queue": "mail"}}]}
/// ```
///
/// `metrics` may also be an object mapping names to values (`{"depth": 120}`)
/// or to metric objects without a `name`.
pub(super) fn parse(text: &str) -> Result<ParsedOutput, ParseError> {
    let doc: Value = serde_json::from_str(text).map_err(|e| ParseError::Json(e.to_string()))?;
    let Value::Object(doc) = doc else {
        return Err(ParseError::Json("expected a JSON object".into()));
    };
    document(&doc).map_err(ParseError::Json)
}

fn document(doc: &Map<String, Value>) -> Result<ParsedOutput, String> {
    let state = match doc.get("state") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => {
            Some(State::parse(s).ok_or_else(|| format!("state: unknown state {s:?}"))?)
        }
        Some(Value::Number(n)) => match n.as_i64() {
            Some(code @ 0..=3) => Some(State::from_exit_code(code as i32)),
            _ => return Err(format!("state: expected 0-3, found {n}")),
        },
        Some(_) => return Err("state: expected a string".into()),
    };

    let metrics = match doc.get("metrics") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                match item {
                    Value::Object(obj) => metric(obj, None),
                    _ => Err("expected an object".into()),
                }
                .map_err(|e| format!("metrics[{i}]: {e}"))
            })
            .collect::<Result<_, _>>()?,
        Some(Value::Object(map)) => map
            .iter()
            .map(|(name, item)| {
                match item {
                    Value::Object(obj) => metric(obj, Some(name)),
                    other => value(other, None).map(|(value, unit, factor)| {
                        Metric::new(name, value * factor).with_unit(unit)
                    }),
                }
                .map_err(|e| format!("metrics.{name}: {e}"))
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("metrics: expected an array or object".into()),
    };

    Ok(ParsedOutput {
        format: Format::Json,
        state,
        message: string_field(doc, "message")?.unwrap_or_default(),
        long_output: string_field(doc, "long_output")?.unwrap_or_default(),
        metrics,
    })
}

/// Parses a metric object. `name` comes from the map key in the shorthand form.
fn metric(obj: &Map<String, Value>, name: Option<&str>) -> Result<Metric, String> {
    let name = match (string_field(obj, "name")?, name) {
        (Some(n), _) => n,
        (None, Some(n)) => n.to_string(),
        (None, None) => return Err("missing name".into()),
    };
    let raw = obj.get("value").ok_or("missing value")?;
    let unit_field = string_field(obj, "unit")?;
    let (value, unit, factor) = value(raw, unit_field.as_deref())?;

    let threshold = |key: &str| {
        match obj.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Threshold::parse_scaled(s, factor).map(Some),
            Some(Value::Number(n)) => Threshold::parse_scaled(&n.to_string(), factor).map(Some),
            Some(_) => return Err(format!("{key}: expected a string or number")),
        }
        .map_err(|e| format!("{key}: {e}"))
    };
    let bound = |key: &str| match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => Ok(n.as_f64().map(|v| v * factor)),
        Some(_) => Err(format!("{key}: expected a number")),
    };

    let mut metric = Metric::new(name, value * factor).with_unit(unit);
    metric.warn = threshold("warn")?;
    metric.crit = threshold("crit")?;
    metric.min = bound("min")?;
    metric.max = bound("max")?;
    match obj.get("labels") {
        None | Some(Value::Null) => {}
        Some(Value::Object(labels)) => {
            for (k, v) in labels {
                let v = match v {
                    Value::String(s) => s.clone(),
                    Value::Number(_) | Value::Bool(_) => v.to_string(),
                    _ => return Err(format!("labels.{k}: expected a scalar")),
                };
                metric.labels.insert(k.clone(), v);
            }
        }
        Some(_) => return Err("labels: expected an object".into()),
    }
    Ok(metric)
}

/// Reads a value given as a number or as a string with unit (`"12ms"`).
/// Returns the raw number, the normalized unit and the conversion factor.
fn value(raw: &Value, unit_field: Option<&str>) -> Result<(f64, Unit, f64), String> {
    let (value, suffix) = match raw {
        Value::Number(n) => (n.as_f64().ok_or("value out of range")?, ""),
        Value::String(s) => split_number(s.trim()).ok_or_else(|| format!("invalid value {s:?}"))?,
        _ => return Err("value: expected a number or string".into()),
    };
    let unit = match (suffix.trim(), unit_field) {
        ("", unit) => unit.unwrap_or(""),
        (suffix, None) => suffix,
        (_, Some(_)) => return Err("unit given both in value and in unit".into()),
    };
    let (unit, factor) = Unit::parse(unit);
    Ok((value, unit, factor))
}

fn string_field(obj: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("{key}: expected a string")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_document() {
        let out = parse(
            r#"{"state": "warning", "message": "queue is filling",
                "long_output": "details",
                "metrics": [{"name": "depth", "value": 120, "labels": {"queue": "mail", "prio": 1},
                             "warn": "> 100", "crit": 500, "min": 0}]}"#,
        )
        .unwrap();
        assert_eq!(out.state, Some(State::Warning));
        assert_eq!(out.message, "queue is filling");
        assert_eq!(out.long_output, "details");
        let m = &out.metrics[0];
        assert_eq!((m.name.as_str(), m.value), ("depth", 120.0));
        assert_eq!(m.labels["queue"], "mail");
        assert_eq!(m.labels["prio"], "1");
        assert_eq!(m.warn, Some(Threshold::parse("> 100").unwrap()));
        assert_eq!(m.crit, Some(Threshold::parse("500").unwrap()));
        assert_eq!(m.min, Some(0.0));
    }

    #[test]
    fn shorthand_metrics() {
        let out = parse(
            r#"{"metrics": {"depth": 120, "latency": "12ms", "disk": {"value": 2, "unit": "GB", "warn": "1.5"}}}"#,
        )
        .unwrap();
        assert_eq!(out.state, None);
        let get = |n: &str| out.metrics.iter().find(|m| m.name == n).unwrap();
        assert_eq!(get("depth").value, 120.0);
        assert_eq!(get("latency").value, 0.012);
        assert_eq!(get("latency").unit, Unit::Seconds);
        let gb = 1024.0 * 1024.0 * 1024.0;
        assert_eq!(get("disk").value, 2.0 * gb);
        // A bare threshold is in the metric's unit.
        assert!(get("disk").warn.as_ref().unwrap().breached(1.6 * gb));
    }

    #[test]
    fn numeric_state() {
        assert_eq!(
            parse(r#"{"state": 2}"#).unwrap().state,
            Some(State::Critical)
        );
        assert!(parse(r#"{"state": 7}"#).is_err());
    }

    #[test]
    fn errors_name_the_field() {
        let err = |s: &str| parse(s).unwrap_err().to_string();
        assert_eq!(err("[1]"), "invalid JSON output: expected a JSON object");
        assert!(err(r#"{"state": "meh"}"#).contains("state"));
        assert!(err(r#"{"metrics": [{"name": "a"}]}"#).contains("metrics[0]: missing value"));
        assert!(err(r#"{"metrics": {"a": "x"}}"#).contains("metrics.a"));
        assert!(err(r#"{"metrics": [{"name": "a", "value": 1, "warn": "?"}]}"#).contains("warn"));
        assert!(
            err(r#"{"metrics": [{"name": "a", "value": "1ms", "unit": "s"}]}"#).contains("unit")
        );
    }
}
