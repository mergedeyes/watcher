use crate::metric::Metric;
use crate::parse::{Format, parse_output};
use crate::state::State;
use crate::threshold::{ConfigThreshold, Threshold};
use crate::unit::format_value;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

/// How a check process ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Termination {
    Exited(i32),
    /// Killed by a signal it did not get from us.
    Signaled(i32),
    TimedOut(Duration),
    /// The process could not be started or waited for.
    Failed(String),
}

impl fmt::Display for Termination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Termination::Exited(code) => write!(f, "check exited with code {code}"),
            Termination::Signaled(sig) => write!(f, "check was killed by signal {sig}"),
            Termination::TimedOut(after) => write!(f, "check timed out after {after:?}"),
            Termination::Failed(err) => write!(f, "check could not be run: {err}"),
        }
    }
}

/// What a check run produced, before any interpretation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawOutput {
    pub termination: Termination,
    pub stdout: String,
    pub stderr: String,
}

/// Thresholds configured for one metric. Each level that is set overrides the
/// same level embedded in the output; an unset level keeps the embedded one.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThresholdSpec {
    pub warn: Option<ConfigThreshold>,
    pub crit: Option<ConfigThreshold>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricResult {
    #[serde(flatten)]
    pub metric: Metric,
    /// `None` when the metric has no thresholds to evaluate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<State>,
    /// Thresholds embedded in the output, for display. They are evaluated
    /// unless the output is Nagios or the config overrides them; the
    /// evaluated ones are `metric.warn`/`metric.crit`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub perf_warn: Option<Threshold>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub perf_crit: Option<Threshold>,
}

/// The evaluated outcome of one check run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckResult {
    /// Final state: the worst of the plugin state and all metric states.
    pub state: State,
    /// State reported by the plugin (exit code, or JSON `state`).
    pub plugin_state: State,
    pub message: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub long_output: String,
    /// The detected output format; `None` if the output was not parsed.
    pub format: Option<Format>,
    pub metrics: Vec<MetricResult>,
}

/// Evaluates a check run: parses the output, applies thresholds and derives
/// the final state.
///
/// Timeouts, signals, exit codes outside 0–3 and unparseable output all give
/// UNKNOWN (or worse, if the exit code says CRITICAL). Thresholds in
/// `thresholds` are keyed by metric name and apply to every label set.
pub fn evaluate(
    raw: &RawOutput,
    format: Format,
    thresholds: &BTreeMap<String, ThresholdSpec>,
) -> CheckResult {
    let code = match raw.termination {
        Termination::Exited(code) => code,
        ref other => {
            return CheckResult {
                state: State::Unknown,
                plugin_state: State::Unknown,
                message: other.to_string(),
                long_output: raw.stdout.trim_end().to_string(),
                format: None,
                metrics: Vec::new(),
            };
        }
    };
    let exit_state = State::from_exit_code(code);

    let parsed = match parse_output(&raw.stdout, format) {
        Ok(parsed) => parsed,
        Err(err) => {
            return CheckResult {
                state: exit_state.worst(State::Unknown),
                plugin_state: exit_state,
                message: err.to_string(),
                long_output: raw.stdout.trim_end().to_string(),
                format: None,
                metrics: Vec::new(),
            };
        }
    };

    let plugin_state = parsed.state.unwrap_or(exit_state);
    // Nagios plugins have already applied their perfdata thresholds to the
    // exit code, and many emit thresholds that don't match the value (e.g.
    // check_disk 3.x: used space with free-space limits). Like Nagios itself,
    // only the exit code counts there.
    let embedded = parsed.format != Format::Nagios;
    let metrics: Vec<MetricResult> = parsed
        .metrics
        .into_iter()
        .map(|metric| apply_thresholds(metric, thresholds, embedded))
        .collect();
    let state = metrics
        .iter()
        .filter_map(|m| m.state)
        .fold(plugin_state, State::worst);

    let message = if !parsed.message.is_empty() {
        parsed.message
    } else if !metrics.is_empty() {
        summarize(&metrics)
    } else if let Some(line) = raw.stderr.lines().map(str::trim).find(|l| !l.is_empty()) {
        line.to_string()
    } else {
        "(no output)".to_string()
    };

    CheckResult {
        state,
        plugin_state,
        message,
        long_output: parsed.long_output,
        format: Some(parsed.format),
        metrics,
    }
}

/// Sets the thresholds to evaluate on `metric`: configured ones, plus embedded
/// ones for levels the config leaves open if `embedded` is set.
fn apply_thresholds(
    mut metric: Metric,
    thresholds: &BTreeMap<String, ThresholdSpec>,
    embedded: bool,
) -> MetricResult {
    let perf_warn = metric.warn.clone();
    let perf_crit = metric.crit.clone();
    if !embedded {
        metric.warn = None;
        metric.crit = None;
    }
    if let Some(spec) = thresholds.get(&metric.name) {
        if let Some(warn) = &spec.warn {
            metric.warn = Some(warn.resolve(&metric.unit));
        }
        if let Some(crit) = &spec.crit {
            metric.crit = Some(crit.resolve(&metric.unit));
        }
    }
    if metric.warn.is_none() && metric.crit.is_none() {
        return MetricResult {
            metric,
            state: None,
            perf_warn,
            perf_crit,
        };
    }
    let breached = |t: &Option<Threshold>| t.as_ref().is_some_and(|t| t.breached(metric.value));
    let state = if breached(&metric.crit) {
        State::Critical
    } else if breached(&metric.warn) {
        State::Warning
    } else {
        State::Ok
    };
    MetricResult {
        metric,
        state: Some(state),
        perf_warn,
        perf_crit,
    }
}

/// `depth=42 latency=12 ms`, for output that has metrics but no message.
fn summarize(metrics: &[MetricResult]) -> String {
    metrics
        .iter()
        .map(|m| {
            format!(
                "{}={}",
                m.metric.series_name(),
                format_value(m.metric.value, &m.metric.unit)
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exited(code: i32, stdout: &str) -> RawOutput {
        RawOutput {
            termination: Termination::Exited(code),
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    fn eval(raw: &RawOutput) -> CheckResult {
        evaluate(raw, Format::Auto, &BTreeMap::new())
    }

    fn spec(warn: &str, crit: &str) -> ThresholdSpec {
        let parse = |s: &str| (!s.is_empty()).then(|| ConfigThreshold::parse(s).unwrap());
        ThresholdSpec {
            warn: parse(warn),
            crit: parse(crit),
        }
    }

    #[test]
    fn exit_code_gives_plugin_state() {
        let r = eval(&exited(1, "LOAD WARNING - load average: 5.1"));
        assert_eq!(r.state, State::Warning);
        assert_eq!(r.format, Some(Format::Nagios));
        assert_eq!(eval(&exited(127, "")).state, State::Unknown);
    }

    #[test]
    fn json_thresholds_raise_the_state() {
        // The plugin says OK, but the value is above its own crit threshold.
        let r = eval(&exited(
            0,
            r#"{"metrics": [{"name": "used", "value": 95, "warn": "> 80", "crit": "> 90"}]}"#,
        ));
        assert_eq!(r.plugin_state, State::Ok);
        assert_eq!(r.metrics[0].state, Some(State::Critical));
        assert_eq!(r.state, State::Critical);
    }

    #[test]
    fn nagios_perfdata_thresholds_are_not_evaluated() {
        // check_disk 3.x: used space, with thresholds on free space.
        let r = eval(&exited(
            0,
            "DISK OK | '/'=746201632768B;204411985100,8;102205992550,4;0;1022059925504",
        ));
        assert_eq!(r.state, State::Ok);
        assert_eq!(r.metrics[0].state, None);
        assert_eq!(r.metrics[0].metric.warn, None);
        // They are kept for display.
        assert_eq!(
            r.metrics[0].perf_warn,
            Some(Threshold::parse("204411985100.8").unwrap())
        );
    }

    #[test]
    fn metrics_without_thresholds_have_no_state() {
        let r = eval(&exited(0, "depth=42"));
        assert_eq!(r.metrics[0].state, None);
    }

    #[test]
    fn configured_thresholds_override_embedded_ones() {
        let json = r#"{"metrics": [{"name": "used", "value": 95, "unit": "%", "warn": "80", "crit": "90"}]}"#;
        let thresholds = BTreeMap::from([("used".to_string(), spec("> 96", "> 98"))]);
        let r = evaluate(&exited(0, json), Format::Auto, &thresholds);
        assert_eq!(r.state, State::Ok);
        assert_eq!(
            r.metrics[0].metric.crit,
            Some(Threshold::parse("> 98").unwrap())
        );

        // Only warn is configured: the embedded crit (0:90) still applies.
        let thresholds = BTreeMap::from([("used".to_string(), spec("> 99", ""))]);
        let r = evaluate(&exited(0, json), Format::Auto, &thresholds);
        assert_eq!(
            r.metrics[0].metric.warn,
            Some(Threshold::parse("> 99").unwrap())
        );
        assert_eq!(r.state, State::Critical);

        // Configured thresholds do apply to Nagios metrics.
        let r = evaluate(&exited(0, "OK | used=95%;99;99"), Format::Auto, &thresholds);
        assert_eq!(r.metrics[0].state, Some(State::Ok));
        let thresholds = BTreeMap::from([("used".to_string(), spec("", "> 90"))]);
        let r = evaluate(&exited(0, "OK | used=95%;99;99"), Format::Auto, &thresholds);
        assert_eq!(r.state, State::Critical);
    }

    #[test]
    fn bare_config_bounds_are_ms_for_time_metrics() {
        let thresholds = BTreeMap::from([("latency".to_string(), spec("> 500", "> 1s"))]);
        let r = evaluate(&exited(0, "latency=600ms"), Format::Auto, &thresholds);
        assert_eq!(r.state, State::Warning);
        let r = evaluate(&exited(0, "latency=1.2s"), Format::Auto, &thresholds);
        assert_eq!(r.state, State::Critical);
    }

    #[test]
    fn json_state_overrides_exit_code() {
        let r = eval(&exited(0, r#"{"state": "critical", "message": "down"}"#));
        assert_eq!(r.plugin_state, State::Critical);
        assert_eq!(r.message, "down");
    }

    #[test]
    fn unparseable_output_is_unknown_without_metrics() {
        let r = eval(&exited(0, "OK | load=abc"));
        assert_eq!(r.state, State::Unknown);
        assert_eq!(r.format, None);
        assert!(r.metrics.is_empty());
        assert!(r.message.contains("perfdata"));
        assert_eq!(r.long_output, "OK | load=abc");
        // A CRITICAL exit code is not downgraded.
        assert_eq!(eval(&exited(2, "{broken}")).state, State::Critical);
    }

    #[test]
    fn abnormal_termination_is_unknown() {
        for termination in [
            Termination::TimedOut(Duration::from_secs(10)),
            Termination::Signaled(9),
            Termination::Failed("No such file or directory".into()),
        ] {
            let raw = RawOutput {
                termination,
                stdout: "OK | a=1".into(),
                stderr: String::new(),
            };
            let r = eval(&raw);
            assert_eq!(r.state, State::Unknown);
            assert!(r.metrics.is_empty());
        }
        let raw = RawOutput {
            termination: Termination::TimedOut(Duration::from_secs(10)),
            stdout: String::new(),
            stderr: String::new(),
        };
        assert_eq!(eval(&raw).message, "check timed out after 10s");
    }

    #[test]
    fn message_fallbacks() {
        assert_eq!(
            eval(&exited(0, "depth=42 latency=12ms")).message,
            "depth=42 latency=12 ms"
        );
        let raw = RawOutput {
            termination: Termination::Exited(127),
            stdout: String::new(),
            stderr: "\nsh: check_foo: not found\n".into(),
        };
        assert_eq!(eval(&raw).message, "sh: check_foo: not found");
        assert_eq!(eval(&exited(0, "")).message, "(no output)");
    }
}
