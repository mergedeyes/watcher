use crate::config_cmd;
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::SystemTime;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;
use watcher_config::{Config, load_dir};
use watcher_core::StateType;
use watcher_exec::scheduler::{Event, Job, schedule};
use watcher_exec::{DEFAULT_OUTPUT_LIMIT, ExecOptions};

#[derive(clap::Args)]
pub struct Args {
    /// watcher.toml, or a directory of object files. Defaults to
    /// ./watcher.toml, then /etc/watcher/watcher.toml.
    path: Option<PathBuf>,

    /// Log every check result, not only state changes.
    #[arg(long)]
    all: bool,

    /// Maximum number of checks running at the same time.
    #[arg(long, default_value_t = 64)]
    max_concurrent: usize,
}

pub async fn run(args: Args) -> Result<ExitCode> {
    let conf_dir = config_cmd::conf_dir(args.path.as_deref())?;
    let Some(config) = load(&conf_dir) else {
        bail!("configuration has errors");
    };
    log(&format!(
        "started with {} services from {} (reload with SIGHUP)",
        config.services.len(),
        conf_dir.display()
    ));

    let (reload_tx, reload_rx) = mpsc::channel(1);
    let (events_tx, mut events) = mpsc::channel(256);
    let scheduler = tokio::spawn(schedule(
        jobs(&config),
        args.max_concurrent,
        reload_rx,
        events_tx,
    ));

    let mut hangup = signal(SignalKind::hangup())?;
    let mut terminate = signal(SignalKind::terminate())?;
    loop {
        tokio::select! {
            Some(event) = events.recv() => print_event(&event, args.all),
            _ = hangup.recv() => {
                if let Some(config) = load(&conf_dir) {
                    log(&format!("reloaded: {} services", config.services.len()));
                    reload_tx.send(jobs(&config)).await?;
                } else {
                    log("reload failed; keeping the running configuration");
                }
            }
            _ = tokio::signal::ctrl_c() => break,
            _ = terminate.recv() => break,
        }
    }
    log("stopping");
    scheduler.abort();
    Ok(ExitCode::SUCCESS)
}

/// Loads the config, printing diagnostics. `None` if it has errors.
fn load(conf_dir: &Path) -> Option<Config> {
    match load_dir(conf_dir) {
        Ok(loaded) => {
            for w in &loaded.warnings {
                log(&w.to_string());
            }
            Some(loaded.config)
        }
        Err(diagnostics) => {
            for d in &diagnostics {
                log(&d.to_string());
            }
            None
        }
    }
}

fn jobs(config: &Config) -> Vec<Job> {
    config
        .services
        .iter()
        .map(|service| {
            let check = config.check(service);
            Job {
                id: service.id.clone(),
                command: service.command.clone(),
                options: ExecOptions {
                    timeout: check.timeout,
                    output_limit: DEFAULT_OUTPUT_LIMIT,
                },
                format: check.format,
                thresholds: check.thresholds.clone(),
                interval: check.interval,
                retry_interval: check.retry_interval,
                max_attempts: check.max_attempts,
            }
        })
        .collect()
}

fn print_event(event: &Event, all: bool) {
    let s = &event.state;
    let kind = match s.state_type {
        StateType::Hard => "HARD".to_string(),
        StateType::Soft => format!("SOFT {}", s.attempt),
    };
    let line = match event.change {
        Some(change) => {
            let marker = if change.hard { "!" } else { " " };
            let what = match change.from {
                None => format!("new -> {} ({kind})", s.state),
                // Only the state type changed: a problem got confirmed.
                Some((state, _)) if state == s.state => format!("{state} SOFT -> {kind}"),
                Some((state, _)) => format!("{state} -> {} ({kind})", s.state),
            };
            format!("{marker} {:<28} {what}", event.id)
        }
        None if all => format!("  {:<28} {} ({kind})", event.id, s.state),
        None => return,
    };
    log(&format!("{line}  {}", event.result.message));
}

fn log(msg: &str) {
    let now = humantime::format_rfc3339_seconds(SystemTime::now());
    println!("{now}  {msg}");
}
