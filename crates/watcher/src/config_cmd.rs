use anyhow::{Result, bail};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use watcher_config::{Bootstrap, CheckDirs, Config, Diagnostic, load_dir};

#[derive(clap::Subcommand)]
pub enum Command {
    /// Validate the configuration and show what it compiles to.
    Check(CheckArgs),
    /// Show where an object and its values come from (the Config Tree).
    Explain(ExplainArgs),
    /// Edit an object file in $VISUAL or $EDITOR, locked against edits in
    /// the web UI while the editor is open.
    Edit(EditArgs),
}

#[derive(clap::Args)]
pub struct EditArgs {
    /// The object file: relative to the object directory (`hosts.toml`),
    /// or a path to it. A new file is created.
    file: PathBuf,

    /// watcher.toml, or a directory of object files.
    #[arg(long, short)]
    config: Option<PathBuf>,

    /// Edit even if someone else is editing the file.
    #[arg(long)]
    force: bool,
}

#[derive(clap::Args)]
pub struct ExplainArgs {
    /// `<host>`, `<host>/<service>`, or `<kind> <name>` with kind one of
    /// host, service, check, hostgroup, servicegroup, contact, contactgroup,
    /// notify.
    #[arg(required = true, num_args = 1..=2)]
    target: Vec<String>,

    /// watcher.toml, or a directory of object files.
    #[arg(long, short)]
    config: Option<PathBuf>,
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
        Command::Explain(args) => explain(args),
        Command::Edit(args) => edit(args),
    }
}

/// `file` relative to the object directory: as given there, or a path
/// to a file inside it.
fn object_path(conf_dir: &Path, file: &Path) -> Result<String> {
    if file.exists()
        || file
            .parent()
            .is_some_and(|p| !p.as_os_str().is_empty() && p.is_dir())
    {
        let dir = conf_dir.canonicalize()?;
        let name = file.file_name().unwrap_or_default();
        let parent = match file.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.canonicalize()?,
            _ => std::env::current_dir()?,
        };
        if let Some(rel) = watcher_config::files::relative(&dir, &parent.join(name)) {
            return Ok(rel);
        }
    }
    let rel = file.to_string_lossy().replace('\\', "/");
    if Path::new(&rel).is_absolute() {
        bail!(
            "{} is not in the object directory {}",
            file.display(),
            conf_dir.display()
        );
    }
    Ok(rel)
}

fn edit(args: EditArgs) -> Result<ExitCode> {
    use watcher_config::locks::{self, Holder, Via};
    let (_, conf_dir) = locate(args.config.as_deref())?;
    let rel = object_path(&conf_dir, &args.file)?;
    let path = watcher_config::files::resolve(&conf_dir, &rel)?;
    let holders = locks::find(&conf_dir, &rel)?;
    if !holders.is_empty() && !args.force {
        for h in &holders {
            eprintln!("{}{}", h.sentence(), since(h));
        }
        bail!("not opened; --force edits it anyway");
    }
    let by = ["SUDO_USER", "USER", "LOGNAME"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .unwrap_or_default();
    let pid = std::process::id();
    let holder = Holder {
        path: rel.clone(),
        by,
        via: Via::Cli,
        since: Some(std::time::SystemTime::now()),
        pid: Some(pid),
        host: locks::local_host(),
    };
    let written = match locks::write(&conf_dir, &rel, &holder, !args.force) {
        // A lock file of a process that is gone is replaced.
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
            if !locks::find(&conf_dir, &rel)?.is_empty() {
                bail!("someone started editing {rel} just now; try again");
            }
            locks::write(&conf_dir, &rel, &holder, false)
        }
        other => other,
    };
    if let Err(err) = written {
        bail!("cannot write the edit lock of {rel}: {err}");
    }
    let status = run_editor(&path);
    locks::remove(&conf_dir, &rel, Via::Cli, pid);
    let status = status?;
    if !status.success() {
        bail!("the editor exited with {status}");
    }
    // What the server will load (it reloads changed files by itself).
    match load_dir(&conf_dir) {
        Ok(loaded) => {
            print_diagnostics(&loaded.warnings);
            println!("{rel}: config OK");
            Ok(ExitCode::SUCCESS)
        }
        Err(diagnostics) => {
            print_diagnostics(&diagnostics);
            eprintln!("\nThe config has errors; the server keeps running the last good one.");
            Ok(ExitCode::FAILURE)
        }
    }
}

/// ` since 10:42`, local time.
fn since(h: &watcher_config::locks::Holder) -> String {
    h.since
        .and_then(|t| jiff::Zoned::try_from(t).ok())
        .map(|z| format!(" since {}", z.strftime("%H:%M")))
        .unwrap_or_default()
}

/// $VISUAL, else $EDITOR, else vi, on `path`.
fn run_editor(path: &Path) -> Result<std::process::ExitStatus> {
    let editor = ["VISUAL", "EDITOR"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty()))
        .unwrap_or_else(|| "vi".into());
    let words = shell_words::split(&editor)?;
    let Some((program, rest)) = words.split_first() else {
        bail!("$EDITOR is empty");
    };
    std::process::Command::new(program)
        .args(rest)
        .arg(path)
        .status()
        .map_err(|e| anyhow::anyhow!("cannot start {program}: {e}"))
}

fn explain(args: ExplainArgs) -> Result<ExitCode> {
    let (kind, name) = match args.target.as_slice() {
        [name] if name.contains('/') => ("service", name.as_str()),
        [name] => ("host", name.as_str()),
        [kind, name] => (kind.as_str(), name.as_str()),
        _ => unreachable!("clap allows one or two values"),
    };
    let (_, conf_dir) = locate(args.config.as_deref())?;
    let config = match load_dir(&conf_dir) {
        Ok(loaded) => loaded.config,
        Err(diagnostics) => {
            print_diagnostics(&diagnostics);
            bail!("the configuration has errors");
        }
    };
    let builder = watcher_config::tree::Builder::new(&config, Some(&conf_dir));
    match builder.tree(kind, name) {
        Some(tree) => {
            let out = &mut io::stdout().lock();
            crate::ignore_broken_pipe(write!(out, "{}", watcher_config::tree::render(&tree)))?;
            Ok(ExitCode::SUCCESS)
        }
        None => bail!("no {kind} {name:?}"),
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
            let out = &mut io::stdout().lock();
            crate::ignore_broken_pipe(
                summary(out, &loaded.config, loaded.warnings.len()).and_then(|()| {
                    if args.services {
                        print_services(out, &loaded.config)?;
                    }
                    Ok(())
                }),
            )?;
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

fn summary(out: &mut impl Write, config: &Config, warnings: usize) -> io::Result<()> {
    let mut parts = vec![
        plural(config.files.len(), "file"),
        plural(config.hosts.len(), "host"),
        plural(config.checks.len(), "check"),
        plural(config.service_objects.len(), "service"),
        format!("{} on hosts", config.services.len()),
    ];
    if !config.contacts.is_empty() {
        parts.push(plural(config.contacts.len(), "contact"));
    }
    if warnings > 0 {
        parts.push(plural(warnings, "warning"));
    }
    writeln!(out, "OK: {}", parts.join(", "))
}

fn print_services(out: &mut impl Write, config: &Config) -> io::Result<()> {
    let width = config
        .services
        .iter()
        .map(|s| s.id.len())
        .max()
        .unwrap_or(0);
    writeln!(out)?;
    for service in &config.services {
        let settings = &service.settings;
        let (timing, command) = match &service.command {
            Some(command) => (
                format!("every {}", humantime::format_duration(settings.interval)),
                command.to_string(),
            ),
            None => {
                let timing = settings.freshness.map_or("passive".to_string(), |f| {
                    format!("fresh {}", humantime::format_duration(f))
                });
                (timing, "(passive)".to_string())
            }
        };
        writeln!(out, "  {:<width$}  {timing:<11}  {command}", service.id)?;
    }
    Ok(())
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}
