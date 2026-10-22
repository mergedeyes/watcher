mod config_cmd;
mod notify_cmd;
mod run;
mod run_check;

use clap::{Parser, Subcommand};
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "watcher",
    version,
    about = "Project Watcher: infrastructure monitoring"
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run a single check and print the evaluated result.
    ///
    /// Exits with the Nagios code of the final state (0 OK, 1 WARNING,
    /// 2 CRITICAL, 3 UNKNOWN), so it can wrap a plugin.
    RunCheck(run_check::Args),

    /// Run all configured checks on their schedule and log state changes.
    ///
    /// Stores results in PostgreSQL when a database is configured. Reloads
    /// the configuration on SIGHUP.
    Run(run::Args),

    /// Notification tools.
    #[command(subcommand)]
    Notify(notify_cmd::Command),

    /// Work with the configuration.
    #[command(subcommand)]
    Config(config_cmd::Command),
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,sqlx=warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .with_writer(std::io::stderr)
        .init();
    // run-check fails with UNKNOWN, like any check that could not be evaluated.
    let (result, failure) = match cli.command {
        Cmd::RunCheck(args) => (run_check::run(args).await, ExitCode::from(3)),
        Cmd::Run(args) => (run::run(args).await, ExitCode::FAILURE),
        Cmd::Config(command) => (config_cmd::run(command), ExitCode::FAILURE),
        Cmd::Notify(command) => (notify_cmd::run(command).await, ExitCode::FAILURE),
    };
    result.unwrap_or_else(|err| {
        eprintln!("error: {err:#}");
        failure
    })
}
