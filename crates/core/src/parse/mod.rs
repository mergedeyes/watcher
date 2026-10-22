//! Check output parsers: Nagios plugin output, `key=value` lines and JSON.

mod json;
mod kv;
mod nagios;

use crate::metric::Metric;
use crate::state::State;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Output format of a check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    /// Detect the format from the output (JSON, then key=value, then Nagios).
    #[default]
    Auto,
    Nagios,
    #[serde(alias = "kv")]
    KeyValue,
    Json,
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Format::Auto => "auto",
            Format::Nagios => "nagios",
            Format::KeyValue => "key_value",
            Format::Json => "json",
        }
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

impl FromStr for Format {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Format::Auto),
            "nagios" => Ok(Format::Nagios),
            "key_value" | "keyvalue" | "kv" => Ok(Format::KeyValue),
            "json" => Ok(Format::Json),
            other => Err(format!(
                "unknown format {other:?} (expected auto, nagios, key_value or json)"
            )),
        }
    }
}

/// The parsed stdout of a check, before thresholds are applied.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedOutput {
    /// The format that was used; never [`Format::Auto`].
    pub format: Format,
    /// State reported in the output itself (JSON `state`). Nagios and
    /// key=value output carry their state in the exit code instead.
    pub state: Option<State>,
    pub message: String,
    pub long_output: String,
    pub metrics: Vec<Metric>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ParseError {
    #[error("invalid JSON output: {0}")]
    Json(String),
    #[error("invalid key=value output on line {line}: {msg}")]
    KeyValue { line: usize, msg: String },
    #[error("invalid perfdata {token:?}: {msg}")]
    Perfdata { token: String, msg: String },
}

/// Parses check stdout in the given format.
///
/// With [`Format::Auto`], output wrapped in `{ … }` is JSON, output where
/// every non-empty line holds only `key=value` tokens is key=value, and
/// anything else is Nagios plugin output.
pub fn parse_output(stdout: &str, format: Format) -> Result<ParsedOutput, ParseError> {
    let text = stdout.replace("\r\n", "\n");
    match format {
        Format::Json => json::parse(&text),
        Format::KeyValue => kv::parse(&text).map(key_value_output),
        Format::Nagios => nagios::parse(&text),
        Format::Auto => {
            let trimmed = text.trim();
            if trimmed.starts_with('{') && trimmed.ends_with('}') {
                return json::parse(trimmed);
            }
            if !trimmed.is_empty()
                && let Ok(metrics) = kv::parse(trimmed)
            {
                return Ok(key_value_output(metrics));
            }
            nagios::parse(&text)
        }
    }
}

fn key_value_output(metrics: Vec<Metric>) -> ParsedOutput {
    ParsedOutput {
        format: Format::KeyValue,
        state: None,
        message: String::new(),
        long_output: String::new(),
        metrics,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_formats() {
        let detect = |s: &str| parse_output(s, Format::Auto).unwrap().format;
        assert_eq!(detect(r#"{"metrics": {"depth": 1}}"#), Format::Json);
        assert_eq!(
            detect("depth=42\nlatency=12ms errors=0\n"),
            Format::KeyValue
        );
        assert_eq!(detect("OK - all good | load=0.5"), Format::Nagios);
        assert_eq!(detect("load is 0.5"), Format::Nagios);
        assert_eq!(detect(""), Format::Nagios);
    }

    #[test]
    fn broken_json_is_an_error_in_auto_mode() {
        assert!(matches!(
            parse_output("{oops}", Format::Auto),
            Err(ParseError::Json(_))
        ));
    }

    #[test]
    fn forced_format_does_not_fall_back() {
        assert!(parse_output("OK - fine", Format::KeyValue).is_err());
        assert!(parse_output("depth=1", Format::Json).is_err());
        // Forced Nagios takes key=value-looking text as a status line.
        let out = parse_output("depth=1", Format::Nagios).unwrap();
        assert_eq!(out.message, "depth=1");
        assert!(out.metrics.is_empty());
    }

    #[test]
    fn crlf_is_normalized() {
        let out = parse_output("OK\r\nline two\r\n", Format::Nagios).unwrap();
        assert_eq!(out.message, "OK");
        assert_eq!(out.long_output, "line two");
    }

    #[test]
    fn format_names() {
        assert_eq!("KV".parse::<Format>(), Ok(Format::KeyValue));
        assert_eq!(Format::KeyValue.to_string(), "key_value");
        assert!("xml".parse::<Format>().is_err());
    }
}
