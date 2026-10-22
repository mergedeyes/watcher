use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::collections::BTreeMap;
use std::process::ExitCode;
use std::time::Duration;
use watcher_core::{
    CheckResult, ConfigThreshold, Format, Termination, Threshold, ThresholdSpec, Unit, evaluate,
    format_value,
};
use watcher_exec::{CommandLine, DEFAULT_OUTPUT_LIMIT, ExecOptions, Execution};

#[derive(clap::Args)]
pub struct Args {
    /// Output format of the check: auto, nagios, key_value or json.
    #[arg(long, default_value = "auto")]
    format: Format,

    /// Kill the check after this long (e.g. 10s, 1m).
    #[arg(long, default_value = "60s", value_parser = humantime::parse_duration)]
    timeout: Duration,

    /// Run the command through /bin/sh -c instead of executing it directly.
    #[arg(long)]
    shell: bool,

    /// Warning threshold for a metric, e.g. 'latency=> 500' or 'load=~:5'.
    /// Bare numbers are milliseconds for time metrics. Repeatable. Overrides
    /// the warning threshold embedded in the output.
    #[arg(long, value_name = "METRIC=THRESHOLD")]
    warn: Vec<String>,

    /// Critical threshold for a metric, like --warn.
    #[arg(long, value_name = "METRIC=THRESHOLD")]
    crit: Vec<String>,

    /// Bytes kept from stdout and from stderr, each.
    #[arg(long, value_name = "BYTES", default_value_t = DEFAULT_OUTPUT_LIMIT)]
    output_limit: usize,

    /// Print the result as JSON.
    #[arg(long)]
    json: bool,

    /// The check command. A single argument is split like a shell would
    /// (without running one); several arguments are used as they are.
    /// Options for run-check must come before it: everything from the
    /// command on is passed to the check.
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    command: Vec<String>,
}

pub async fn run(args: Args) -> Result<ExitCode> {
    let command = match args.command.as_slice() {
        [single] => CommandLine::parse(single, args.shell)?,
        many if args.shell => CommandLine::Shell(many.join(" ")),
        many => CommandLine::Argv(many.to_vec()),
    };
    let thresholds = thresholds(&args.warn, &args.crit)?;
    let opts = ExecOptions {
        timeout: args.timeout,
        output_limit: args.output_limit,
    };

    let exec = watcher_exec::run(&command, &opts).await;
    let result = evaluate(&exec.output, args.format, &thresholds);

    if args.json {
        print_json(&result, &exec)?;
    } else {
        print_human(&result, &exec);
    }
    Ok(ExitCode::from(result.state.exit_code() as u8))
}

/// Collects `--warn`/`--crit` flags into per-metric threshold specs.
fn thresholds(warn: &[String], crit: &[String]) -> Result<BTreeMap<String, ThresholdSpec>> {
    let mut specs: BTreeMap<String, ThresholdSpec> = BTreeMap::new();
    for (flag, items) in [("--warn", warn), ("--crit", crit)] {
        for item in items {
            let Some((metric, spec)) = item.split_once('=') else {
                bail!("{flag} {item:?}: expected METRIC=THRESHOLD");
            };
            let threshold =
                ConfigThreshold::parse(spec).with_context(|| format!("{flag} {item:?}"))?;
            let entry = specs.entry(metric.trim().to_string()).or_default();
            if flag == "--warn" {
                entry.warn = Some(threshold);
            } else {
                entry.crit = Some(threshold);
            }
        }
    }
    Ok(specs)
}

fn print_json(result: &CheckResult, exec: &Execution) -> Result<()> {
    #[derive(Serialize)]
    struct Output<'a> {
        #[serde(flatten)]
        result: &'a CheckResult,
        duration_ms: f64,
        #[serde(skip_serializing_if = "Option::is_none")]
        stderr: Option<&'a str>,
    }
    let out = Output {
        result,
        duration_ms: exec.duration.as_micros() as f64 / 1000.0,
        stderr: Some(exec.output.stderr.trim_end()).filter(|s| !s.is_empty()),
    };
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

fn print_human(result: &CheckResult, exec: &Execution) {
    println!("{} - {}", result.state, result.message);
    for line in result.long_output.lines() {
        println!("  {line}");
    }

    if !result.metrics.is_empty() {
        let rows: Vec<[String; 5]> = result
            .metrics
            .iter()
            .map(|m| {
                let unit = &m.metric.unit;
                let threshold = |t: &Option<Threshold>| {
                    t.as_ref().map(|t| t.describe(unit)).unwrap_or_default()
                };
                [
                    m.metric.series_name(),
                    format_value(m.metric.value, unit),
                    threshold(&m.metric.warn),
                    threshold(&m.metric.crit),
                    m.state.map(|s| s.to_string()).unwrap_or_default(),
                ]
            })
            .collect();
        let header = ["METRIC", "VALUE", "WARN", "CRIT", "STATE"].map(String::from);
        let mut widths = [0; 5];
        for row in std::iter::once(&header).chain(&rows) {
            for (w, cell) in widths.iter_mut().zip(row) {
                *w = (*w).max(cell.chars().count());
            }
        }
        println!();
        for row in std::iter::once(&header).chain(&rows) {
            let line: Vec<String> = row
                .iter()
                .zip(widths)
                .map(|(cell, w)| format!("{cell:<w$}"))
                .collect();
            println!("  {}", line.join("  ").trim_end());
        }
    }

    let stderr = exec.output.stderr.trim_end();
    if !stderr.is_empty() {
        println!("\nstderr:");
        for line in stderr.lines() {
            println!("  {line}");
        }
    }

    let mut facts = Vec::new();
    // Other terminations are already the message.
    if let Termination::Exited(code) = exec.output.termination {
        facts.push(format!("exit code {code}"));
    }
    if let Some(format) = result.format {
        facts.push(format!("format {format}"));
    }
    facts.push(format!(
        "took {}",
        format_value(exec.duration.as_secs_f64(), &Unit::Seconds)
    ));
    if exec.stdout_truncated || exec.stderr_truncated {
        facts.push("output truncated".into());
    }
    println!("\n{}", facts.join(" · "));
}
