mod config_cmd;
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
    /// Reloads the configuration on SIGHUP. Nothing is stored yet.
    Run(run::Args),

    /// Work with the configuration.
    #[command(subcommand)]
    Config(config_cmd::Command),
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    // run-check fails with UNKNOWN, like any check that could not be evaluated.
    let (result, failure) = match cli.command {
        Cmd::RunCheck(args) => (run_check::run(args).await, ExitCode::from(3)),
        Cmd::Run(args) => (run::run(args).await, ExitCode::FAILURE),
        Cmd::Config(command) => (config_cmd::run(command), ExitCode::FAILURE),
    };
    result.unwrap_or_else(|err| {
        eprintln!("error: {err:#}");
        failure
    })
}
