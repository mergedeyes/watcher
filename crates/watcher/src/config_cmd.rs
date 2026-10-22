use anyhow::{Result, bail};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use watcher_config::{Bootstrap, CheckDirs, Config, Diagnostic, load_dir};

#[derive(clap::Subcommand)]
pub enum Command {
    /// Validate the configuration and show what it compiles to.
    Check(CheckArgs),
}

#[derive(clap::Args)]
pub struct CheckArgs {
    /// watcher.toml, or a directory of object files. Defaults to
    /// ./watcher.toml, then /etc/watcher/watcher.toml.
    path: Option<PathBuf>,

    /// List every service with its expanded command.
    #[arg(long)]
    services: bool,
}

pub fn run(command: Command) -> Result<ExitCode> {
    match command {
        Command::Check(args) => check(args),
    }
}

fn check(args: CheckArgs) -> Result<ExitCode> {
    let (bootstrap, conf_dir) = locate(args.path.as_deref())?;
    match load_dir(&conf_dir) {
        Ok(mut loaded) => {
            let check_dirs = check_dirs(bootstrap.as_ref(), &conf_dir);
            loaded
                .warnings
                .extend(check_dirs.missing_commands(&loaded.config));
            print_diagnostics(&loaded.warnings);
            summary(&loaded.config, loaded.warnings.len());
            if args.services {
                print_services(&loaded.config);
            }
            Ok(ExitCode::SUCCESS)
        }
        Err(diagnostics) => {
            print_diagnostics(&diagnostics);
            let errors = diagnostics
                .iter()
                .filter(|d| d.severity == watcher_config::Severity::Error)
                .count();
            eprintln!("\n{} in {}", plural(errors, "error"), conf_dir.display());
            Ok(ExitCode::FAILURE)
        }
    }
}

/// Where commands are looked up: from watcher.toml, or the defaults next
/// to the object directory.
pub fn check_dirs(bootstrap: Option<&Bootstrap>, conf_dir: &Path) -> CheckDirs {
    match bootstrap {
        Some(b) => b.check_dirs(),
        None => CheckDirs::default_for(conf_dir.parent().unwrap_or(Path::new("."))),
    }
}

/// Finds the bootstrap file and object directory. A directory path is used
/// as the object directory directly, without a bootstrap file.
pub fn locate(path: Option<&Path>) -> Result<(Option<Bootstrap>, PathBuf)> {
    let path = match path {
        Some(p) if p.is_dir() => return Ok((None, p.to_path_buf())),
        Some(p) => p.to_path_buf(),
        None => {
            let candidates = [
                Path::new("watcher.toml"),
                Path::new("/etc/watcher/watcher.toml"),
            ];
            match candidates.iter().find(|p| p.exists()) {
                Some(p) => p.to_path_buf(),
                None => {
                    bail!("no watcher.toml in the current directory or /etc/watcher; pass a path")
                }
            }
        }
    };
    let bootstrap = Bootstrap::load(&path)?;
    let conf_dir = bootstrap.conf_dir.clone();
    Ok((Some(bootstrap), conf_dir))
}

fn print_diagnostics(diagnostics: &[Diagnostic]) {
    for d in diagnostics {
        eprintln!("{d}");
    }
}

fn summary(config: &Config, warnings: usize) {
    let mut parts = vec![
        plural(config.files.len(), "file"),
        plural(config.hosts.len(), "host"),
        plural(config.checks.len(), "check"),
        plural(config.services.len(), "service"),
    ];
    if !config.contacts.is_empty() {
        parts.push(plural(config.contacts.len(), "contact"));
    }
    if warnings > 0 {
        parts.push(plural(warnings, "warning"));
    }
    println!("OK: {}", parts.join(", "));
}

fn print_services(config: &Config) {
    let width = config
        .services
        .iter()
        .map(|s| s.id.len())
        .max()
        .unwrap_or(0);
    println!();
    for service in &config.services {
        let check = config.check(service);
        let (timing, command) = match &service.command {
            Some(command) => (
                format!("every {}", humantime::format_duration(check.interval)),
                command.to_string(),
            ),
            None => {
                let timing = check.freshness.map_or("passive".to_string(), |f| {
                    format!("fresh {}", humantime::format_duration(f))
                });
                (timing, "(passive)".to_string())
            }
        };
        println!("  {:<width$}  {timing:<11}  {command}", service.id);
    }
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}
