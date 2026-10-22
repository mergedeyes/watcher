use crate::config_cmd;
use anyhow::{Context, Result, bail};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::SystemTime;
use watcher_config::load_dir;
use watcher_core::State;
use watcher_notify::{Kind, Notification, Notifier};

#[derive(clap::Subcommand)]
pub enum Command {
    /// Send a test notification to a contact over all its channels.
    Test(TestArgs),
}

#[derive(clap::Args)]
pub struct TestArgs {
    /// Name of the contact, as in `[contact.<name>]`.
    contact: String,

    /// watcher.toml, or a directory of object files. Defaults to
    /// ./watcher.toml, then /etc/watcher/watcher.toml.
    path: Option<PathBuf>,
}

pub async fn run(command: Command) -> Result<ExitCode> {
    match command {
        Command::Test(args) => test(args).await,
    }
}

async fn test(args: TestArgs) -> Result<ExitCode> {
    let (bootstrap, conf_dir) = config_cmd::locate(args.path.as_deref())?;
    let config = match load_dir(&conf_dir) {
        Ok(loaded) => loaded.config,
        Err(diagnostics) => {
            for d in &diagnostics {
                eprintln!("{d}");
            }
            bail!("configuration has errors");
        }
    };
    let Some(contact) = config.contacts.iter().find(|c| c.name == args.contact) else {
        bail!("no contact {:?} in {}", args.contact, conf_dir.display());
    };
    let smtp = bootstrap.as_ref().and_then(|b| b.smtp.as_ref());
    let public_url = bootstrap.as_ref().and_then(|b| b.public_url.clone());
    let notifier = Notifier::new(smtp, public_url.clone()).context("invalid [smtp] settings")?;

    let notification = Notification {
        kind: Kind::Test,
        host: "watcher".into(),
        address: "watcher".into(),
        service: "test".into(),
        state: State::Ok,
        previous_state: None,
        attempts: 1,
        output: format!(
            "If you can read this, notifications to {} work.",
            contact.name
        ),
        long_output: String::new(),
        time: SystemTime::now(),
        url: public_url,
    };
    let results = notifier.deliver(contact, &notification).await;
    if results.is_empty() {
        bail!("contact {:?} has no email address or webhook", contact.name);
    }
    let mut ok = true;
    for (channel, result) in results {
        match result {
            Ok(()) => println!("{channel}: sent"),
            Err(err) => {
                ok = false;
                println!("{channel}: failed: {err}");
            }
        }
    }
    Ok(if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
