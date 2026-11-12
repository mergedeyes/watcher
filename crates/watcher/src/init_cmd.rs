//! `watcher init`: writes watcher.toml and a first conf.d/, interactively or
//! from an answers file.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::io::{BufRead, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use watcher_config::{
    Acme, Bootstrap, Contact, InstanceRole, LETS_ENCRYPT, Smtp, SmtpTls, Tls, load_dir,
};
use watcher_exec::ssh::Sealer;
use watcher_hub::auth::{self, hash_password};
use watcher_hub::totp;
use watcher_notify::{Mailer, Notification, Notifier};
use watcher_store::Store;

pub(crate) use watcher_hub::auth::MIN_PASSWORD;

#[derive(clap::Args)]
pub struct Args {
    /// Directory for watcher.toml and conf.d/.
    #[arg(default_value = ".")]
    dir: PathBuf,

    /// Take the answers from a TOML file instead of asking.
    #[arg(long, value_name = "FILE")]
    answers: Option<PathBuf>,

    /// Overwrite existing files.
    #[arg(long)]
    force: bool,
}

/// Everything init asks. Also the format of `--answers`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Answers {
    #[serde(default)]
    pub role: InstanceRole,
    pub instance_name: Option<String>,
    pub listen: Option<String>,
    pub public_url: Option<String>,
    /// IANA time zone of downtime windows and time periods, like
    /// `Europe/Berlin`.
    pub timezone: Option<String>,
    /// HTTPS: certificate files or self-signed.
    pub tls: Option<Tls>,
    pub database_url: Option<String>,
    pub admin: Option<Admin>,
    pub smtp: Option<Smtp>,
    #[serde(default)]
    pub alert_email: Vec<String>,
    pub alert_webhook: Option<String>,
    #[serde(default = "yes")]
    pub monitor_localhost: bool,
    #[serde(default)]
    pub test_notification: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Admin {
    pub username: String,
    pub password: Option<String>,
    pub password_file: Option<PathBuf>,
    /// Turns on two-factor authentication with this key (base32), the one
    /// the authenticator app has.
    pub totp_secret: Option<String>,
}

fn yes() -> bool {
    true
}

pub async fn run(args: Args) -> Result<ExitCode> {
    let interactive = args.answers.is_none();
    let answers = match &args.answers {
        Some(file) => {
            let text = std::fs::read_to_string(file)
                .with_context(|| format!("cannot read {}", file.display()))?;
            toml::from_str(&text).with_context(|| format!("{}", file.display()))?
        }
        None => {
            let stdin = std::io::stdin();
            let tty = std::io::IsTerminal::is_terminal(&stdin);
            let mut input = stdin.lock();
            let mut output = std::io::stdout();
            println!("Watcher setup: {}\n", args.dir.display());
            let mut p = Prompter {
                input: &mut input,
                output: &mut output,
                tty,
            };
            ask(&mut p).await?
        }
    };

    let plan = Plan::new(&answers, &args.dir)?;
    let existing: Vec<&PathBuf> = plan
        .files
        .iter()
        .map(|f| &f.path)
        .filter(|p| p.exists())
        .collect();
    if !existing.is_empty() && !args.force {
        bail!(
            "exists: {} (use --force to overwrite)",
            existing
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    println!("\nWrite:");
    for file in &plan.files {
        println!("  {}", file.path.display());
    }
    if let Some(admin) = &answers.admin {
        let totp = if admin.totp_secret.is_some() {
            ", two-factor"
        } else {
            ""
        };
        println!("Create user: {} (admin{totp})", admin.username);
    }
    if interactive {
        let stdin = std::io::stdin();
        let tty = std::io::IsTerminal::is_terminal(&stdin);
        let mut input = stdin.lock();
        let mut output = std::io::stdout();
        let mut p = Prompter {
            input: &mut input,
            output: &mut output,
            tty,
        };
        if !p.yes("Proceed?", true)? {
            println!("Aborted.");
            return Ok(ExitCode::FAILURE);
        }
    }

    let recovery = apply(&answers, &plan).await?;
    if let Some(admin) = answers.admin.as_ref().filter(|_| !recovery.is_empty()) {
        println!(
            "\nRecovery codes for {} (shown only now; each works once, in place of a code from the app):",
            admin.username
        );
        for code in &recovery {
            println!("  {code}");
        }
    }
    let config = args.dir.join("watcher.toml");
    println!("\nDone. Start: watcher run {}", config.display());
    Ok(ExitCode::SUCCESS)
}

/// Asks everything, with defaults in brackets.
async fn ask(p: &mut Prompter<'_>) -> Result<Answers> {
    let mut a = Answers {
        monitor_localhost: true,
        ..Answers::default()
    };

    a.role = loop {
        match p.ask("Role (standalone, hub)", "standalone")?.as_str() {
            "standalone" => break InstanceRole::Standalone,
            "hub" => break InstanceRole::Hub,
            "node" => p.note("node role: not available yet")?,
            other => p.note(&format!("unknown role: {other}"))?,
        }
    };

    // Database first: whether logins exist decides which addresses are allowed.
    a.database_url = loop {
        let Some(url) = p.ask_opt("Database URL", "postgres://watcher@localhost/watcher")? else {
            break None;
        };
        match Store::connect(&url).await {
            Ok(store) => {
                p.note(&format!("OK: {}, schema up to date", store.backend()))?;
                break Some(url);
            }
            Err(err) => p.note(&format!("error: {err}"))?,
        }
    };

    let listen = loop {
        let value = p.ask("Listen address", "127.0.0.1:8080")?;
        match value.parse::<SocketAddr>() {
            Ok(addr) if !addr.ip().is_loopback() && a.database_url.is_none() => {
                p.note("error: non-loopback addresses need a database (for logins)")?
            }
            Ok(addr) => break addr,
            Err(_) => p.note(&format!("error: invalid address: {value}"))?,
        }
    };
    a.listen = Some(listen.to_string());
    let host = if listen.ip().is_unspecified() {
        hostname()
    } else {
        listen.ip().to_string()
    };
    a.public_url = Some(p.ask("Public URL", &format!("http://{host}:{}", listen.port()))?);
    a.timezone = loop {
        let value = p.ask("Timezone", &system_timezone())?;
        match jiff::tz::TimeZone::get(&value) {
            Ok(_) => break Some(value),
            Err(_) => p.note(&format!(
                "error: unknown time zone {value:?} (an IANA name like Europe/Berlin)"
            ))?,
        }
    };

    if a.database_url.is_some() {
        let username = p.ask("Admin user", "admin")?;
        let password = loop {
            let first = p.password("Admin password: ")?;
            if first.chars().count() < MIN_PASSWORD {
                p.note(&format!("error: min. {MIN_PASSWORD} characters"))?;
                continue;
            }
            if p.password("Repeat password: ")? != first {
                p.note("error: passwords differ")?;
                continue;
            }
            break first;
        };
        let totp_secret = ask_totp(p, &username, &totp::new_secret())?;
        a.admin = Some(Admin {
            username,
            password: Some(password),
            password_file: None,
            totp_secret,
        });
    }

    if let Some(host) = p.ask_opt("SMTP host", "")? {
        let tls = loop {
            match p
                .ask("SMTP TLS (starttls, tls, none)", "starttls")?
                .as_str()
            {
                "starttls" => break SmtpTls::Starttls,
                "tls" => break SmtpTls::Tls,
                "none" => break SmtpTls::None,
                other => p.note(&format!("unknown: {other}"))?,
            }
        };
        let default_port = Smtp {
            host: String::new(),
            port: None,
            tls,
            username: None,
            password: None,
            password_file: None,
            from: String::new(),
        }
        .port();
        let port = loop {
            match p
                .ask("SMTP port", &default_port.to_string())?
                .parse::<u16>()
            {
                Ok(port) => break port,
                Err(_) => p.note("error: invalid port")?,
            }
        };
        let username = p.ask_opt("SMTP user", "")?;
        let password = match &username {
            Some(_) => Some(p.password("SMTP password: ")?),
            None => None,
        };
        let from = p.ask("From address", &format!("watcher@{}", hostname()))?;
        a.smtp = Some(Smtp {
            host,
            port: Some(port),
            tls,
            username,
            password,
            password_file: None,
            from,
        });
    }

    if let Some(emails) = p.ask_opt("Alert email(s), comma-separated", "")? {
        a.alert_email = emails
            .split(',')
            .map(|e| e.trim().to_string())
            .filter(|e| !e.is_empty())
            .collect();
    }
    a.alert_webhook = p.ask_opt("Alert webhook URL", "")?;
    if (!a.alert_email.is_empty() && a.smtp.is_some()) || a.alert_webhook.is_some() {
        a.test_notification = p.yes("Send test notification?", true)?;
    }

    a.monitor_localhost = p.yes("Monitor this machine?", true)?;
    if a.monitor_localhost {
        match plugin_dir() {
            Some(dir) => p.note(&format!("plugins: {}", dir.display()))?,
            None => p.note("plugins: not found, using shell checks")?,
        }
    }
    Ok(a)
}

/// Offers two-factor authentication for the admin with `secret`: shows the
/// QR code and the key, then takes a code from the app, which shows the app
/// is set up right. Returns the key (base32), or `None` if declined.
fn ask_totp(p: &mut Prompter<'_>, username: &str, secret: &[u8]) -> Result<Option<String>> {
    if !p.yes("Two-factor authentication (TOTP)?", false)? {
        return Ok(None);
    }
    let key = totp::encode(secret);
    let qr = totp::qr_text(&totp::uri("Watcher", username, secret)).map_err(anyhow::Error::msg)?;
    writeln!(p.output, "\n{qr}")?;
    p.note("Scan with an authenticator app, or enter the key:")?;
    let groups: Vec<&str> = key
        .as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).unwrap_or_default())
        .collect();
    p.note(&groups.join(" "))?;
    loop {
        let Some(code) = p.ask_opt("Code from the app", "")? else {
            p.note("two-factor authentication: skipped")?;
            return Ok(None);
        };
        match totp::verify(secret, &code, totp::now()) {
            Some(_) => return Ok(Some(key)),
            None => p.note("error: invalid code; is the clock of the device right?")?,
        }
    }
}

/// Terminal prompts. Without a terminal, passwords are read as plain lines,
/// so answers can be piped in.
struct Prompter<'a> {
    input: &'a mut dyn BufRead,
    output: &'a mut dyn Write,
    tty: bool,
}

impl Prompter<'_> {
    fn line(&mut self, prompt: &str) -> Result<String> {
        write!(self.output, "{prompt}")?;
        self.output.flush()?;
        let mut line = String::new();
        if self.input.read_line(&mut line)? == 0 {
            bail!("input ended");
        }
        if !self.tty {
            // Echo piped input so the transcript reads like a session.
            writeln!(self.output)?;
        }
        Ok(line.trim().to_string())
    }

    fn note(&mut self, text: &str) -> Result<()> {
        writeln!(self.output, "  {text}")?;
        Ok(())
    }

    /// `Label [default]: `; empty input takes the default.
    fn ask(&mut self, label: &str, default: &str) -> Result<String> {
        let answer = self.line(&format!("{label} [{default}]: "))?;
        Ok(if answer.is_empty() {
            default.to_string()
        } else {
            answer
        })
    }

    /// Like [`ask`](Self::ask), but `none` (or empty without default) skips.
    fn ask_opt(&mut self, label: &str, default: &str) -> Result<Option<String>> {
        let shown = if default.is_empty() { "none" } else { default };
        let answer = self.line(&format!("{label} [{shown}]: "))?;
        let value = if answer.is_empty() {
            default.to_string()
        } else {
            answer
        };
        Ok((!value.is_empty() && value != "none").then_some(value))
    }

    fn yes(&mut self, label: &str, default: bool) -> Result<bool> {
        loop {
            let hint = if default { "Y/n" } else { "y/N" };
            match self
                .line(&format!("{label} [{hint}]: "))?
                .to_lowercase()
                .as_str()
            {
                "" => return Ok(default),
                "y" | "yes" => return Ok(true),
                "n" | "no" => return Ok(false),
                _ => self.note("answer y or n")?,
            }
        }
    }

    fn password(&mut self, prompt: &str) -> Result<String> {
        if self.tty {
            Ok(rpassword::prompt_password(prompt)?)
        } else {
            self.line(prompt)
        }
    }
}

/// A file to write.
#[derive(Debug)]
pub(crate) struct PlannedFile {
    pub path: PathBuf,
    pub contents: String,
    /// Holds secrets: mode 0600.
    pub private: bool,
}

/// The validated result of the answers: files and the admin to create.
#[derive(Debug)]
pub(crate) struct Plan {
    pub files: Vec<PlannedFile>,
    conf_dir: PathBuf,
    /// Where `secret.key` goes (watcher.toml keeps the default `data`).
    data_dir: PathBuf,
}

impl Plan {
    pub(crate) fn new(a: &Answers, dir: &Path) -> Result<Self> {
        if a.role == InstanceRole::Node {
            bail!("role node: not available yet");
        }
        let listen: SocketAddr = a
            .listen
            .as_deref()
            .unwrap_or("127.0.0.1:8080")
            .parse()
            .context("listen")?;
        if !listen.ip().is_loopback() && a.database_url.is_none() {
            bail!("listen {listen}: non-loopback addresses need database_url (for logins)");
        }
        if let Some(url) = &a.public_url
            && !(url.starts_with("http://") || url.starts_with("https://"))
        {
            bail!("public_url: must start with http:// or https://");
        }
        // ACME with its domains filled in from public_url, as watcher run
        // would; written out so watcher.toml says which names it covers.
        let mut acme = None;
        if let Some(tls) = &a.tls {
            match (&tls.cert, &tls.key, tls.self_signed, &tls.acme) {
                (Some(cert), Some(key), false, None) => {
                    for (what, file) in [("cert", cert), ("key", key)] {
                        if !file.is_file() {
                            bail!("tls {what}: no file {}", file.display());
                        }
                    }
                }
                (None, None, true, None) => {}
                (None, None, false, Some(settings)) => {
                    let mut settings = settings.clone();
                    settings
                        .resolve(a.public_url.as_deref())
                        .map_err(anyhow::Error::msg)?;
                    acme = Some(settings);
                }
                _ => bail!("tls: set cert and key, self_signed = true, or acme"),
            }
        }
        if let Some(name) = &a.instance_name
            && name.trim().is_empty()
        {
            bail!("instance_name: empty");
        }
        if let Some(tz) = &a.timezone
            && jiff::tz::TimeZone::get(tz).is_err()
        {
            bail!("timezone {tz:?}: unknown (an IANA name like Europe/Berlin)");
        }
        if a.admin.is_some() && a.database_url.is_none() {
            bail!("admin: needs database_url");
        }
        if let Some(admin) = &a.admin {
            let password = admin_password(admin)?;
            if password.chars().count() < MIN_PASSWORD {
                bail!("admin password: min. {MIN_PASSWORD} characters");
            }
            if let Some(key) = &admin.totp_secret
                && totp::decode(key).is_none()
            {
                bail!("admin totp_secret: not a TOTP key (base32, at least 26 characters)");
            }
        }
        if let Some(smtp) = &a.smtp {
            Mailer::new(smtp).map_err(|e| anyhow::anyhow!("smtp: {e}"))?;
        }
        for email in &a.alert_email {
            if !email.contains('@') {
                bail!("alert_email: invalid address {email:?}");
            }
        }
        if let Some(url) = &a.alert_webhook
            && !(url.starts_with("http://") || url.starts_with("https://"))
        {
            bail!("alert_webhook: must be an http(s) URL");
        }

        let conf_dir = dir.join("conf.d");
        let mut files = Vec::new();
        let mut toml = format!(
            "# Written by watcher init.\nrole = {}\nconf_dir = \"conf.d\"\nlisten = {}\n\
             # Bare command names are looked up here first, then in PATH.\n\
             # Default: checks/, then the monitoring-plugins directories.\n\
             # check_dirs = [\"checks\", \"/usr/lib/nagios/plugins\"]\n",
            q(a.role.as_str()),
            q(&listen.to_string())
        );
        if let Some(url) = &a.public_url {
            toml += &format!("public_url = {}\n", q(url));
        }
        if let Some(name) = &a.instance_name {
            toml += &format!("instance_name = {}\n", q(name.trim()));
        }
        if let Some(tz) = &a.timezone {
            toml += &format!(
                "# Time zone of downtime windows and time periods.\ntimezone = {}\n",
                q(tz)
            );
        }
        if let Some(acme) = &acme {
            toml += &acme_section(acme);
        } else if let Some(tls) = &a.tls {
            toml += "\n[tls]\n";
            match (&tls.cert, &tls.key) {
                (Some(cert), Some(key)) => {
                    toml += &format!(
                        "cert = {}\nkey = {}\n",
                        q(&cert.display().to_string()),
                        q(&key.display().to_string())
                    );
                }
                _ => toml += "self_signed = true\n",
            }
        }
        if let Some(url) = &a.database_url {
            toml += &format!("\n[database]\nurl = {}\n", q(url));
        }
        if let Some(smtp) = &a.smtp {
            toml += &format!(
                "\n[smtp]\nhost = {}\nport = {}\ntls = {}\n",
                q(&smtp.host),
                smtp.port(),
                q(tls_str(smtp.tls))
            );
            if let Some(user) = &smtp.username {
                toml += &format!("username = {}\n", q(user));
            }
            match (&smtp.password, &smtp.password_file) {
                (Some(password), _) => {
                    toml += "password_file = \"smtp.pass\"\n";
                    files.push(PlannedFile {
                        path: dir.join("smtp.pass"),
                        contents: format!("{password}\n"),
                        private: true,
                    });
                }
                (None, Some(file)) => {
                    toml += &format!("password_file = {}\n", q(&file.display().to_string()))
                }
                (None, None) => {}
            }
            toml += &format!("from = {}\n", q(&smtp.from));
        }
        files.insert(
            0,
            PlannedFile {
                path: dir.join("watcher.toml"),
                contents: toml,
                private: true,
            },
        );

        let contact = !a.alert_email.is_empty() || a.alert_webhook.is_some();
        if contact {
            let mut c = "[contact.admins]\n".to_string();
            if !a.alert_email.is_empty() {
                let list: Vec<String> = a.alert_email.iter().map(|e| q(e)).collect();
                c += &format!("email = [{}]\n", list.join(", "));
            }
            if let Some(url) = &a.alert_webhook {
                c += &format!("webhook = {}\n", q(url));
            }
            files.push(PlannedFile {
                path: conf_dir.join("contacts.toml"),
                contents: c,
                private: false,
            });
            files.push(PlannedFile {
                path: conf_dir.join("notify.toml"),
                contents:
                    "# Every matching rule notifies. Narrow with hosts, services and states.\n\
                           [[notify]]\nname = \"admins\"\ncontacts = [\"admins\"]\n"
                        .into(),
                private: false,
            });
        }
        if a.monitor_localhost {
            let (checks, services, hosts) = localhost_config();
            for (name, contents) in [
                ("checks.toml", checks),
                ("services.toml", services),
                ("hosts.toml", hosts),
            ] {
                files.push(PlannedFile {
                    path: conf_dir.join(name),
                    contents,
                    private: false,
                });
            }
        }
        Ok(Plan {
            files,
            conf_dir,
            data_dir: dir.join("data"),
        })
    }
}

/// `[tls.acme]` for watcher.toml; the directory only if it isn't the
/// default.
fn acme_section(acme: &Acme) -> String {
    let domains: Vec<String> = acme.domains.iter().map(|d| q(d)).collect();
    let mut out = format!(
        "\n# HTTPS certificates from an ACME CA. It connects to port 443 of each\n\
         # domain to check it, which has to reach this server.\n\
         [tls.acme]\ndomains = [{}]\n",
        domains.join(", ")
    );
    if let Some(email) = &acme.email {
        out += &format!("email = {}\n", q(email));
    }
    if acme.directory != LETS_ENCRYPT {
        out += &format!("directory = {}\n", q(&acme.directory));
    }
    if let Some(ca) = &acme.ca {
        out += &format!("ca = {}\n", q(&ca.display().to_string()));
    }
    if let (Some(kid), Some(key)) = (&acme.eab_kid, &acme.eab_hmac_key) {
        out += &format!("eab_kid = {}\neab_hmac_key = {}\n", q(kid), q(key));
    }
    out
}

pub(crate) fn admin_password(admin: &Admin) -> Result<String> {
    match (&admin.password, &admin.password_file) {
        (Some(p), None) => Ok(p.clone()),
        (None, Some(file)) => Ok(std::fs::read_to_string(file)
            .with_context(|| format!("admin password_file {}", file.display()))?
            .trim_end_matches(['\r', '\n'])
            .to_string()),
        _ => bail!("admin: set password or password_file"),
    }
}

/// Creates the admin, writes the files, checks they load, sends the test.
/// Returns the admin's recovery codes if two-factor authentication was
/// turned on.
pub(crate) async fn apply(a: &Answers, plan: &Plan) -> Result<Vec<String>> {
    let mut recovery = Vec::new();
    if let Some(url) = &a.database_url {
        let store = Store::connect(url).await.context("database")?;
        println!("Database: {}", store.backend());
        if let Some(admin) = &a.admin {
            let hash = hash_password(&admin_password(admin)?).map_err(anyhow::Error::msg)?;
            match store.create_user(&admin.username, &hash, true).await {
                Ok(user) => {
                    println!("User: {} created", admin.username);
                    if let Some(secret) = admin.totp_secret.as_deref().and_then(totp::decode) {
                        recovery = enable_totp(&store, &plan.data_dir, user.id, &secret).await?;
                        println!("Two-factor authentication: on");
                    }
                }
                Err(watcher_store::Error::Conflict(_)) => {
                    println!("User: {} exists, unchanged", admin.username)
                }
                Err(err) => return Err(err.into()),
            }
        }
    }

    // conf.d may get no file (no contact, this machine not monitored), but
    // it has to exist for the check below and for `watcher run`.
    std::fs::create_dir_all(&plan.conf_dir)
        .with_context(|| format!("cannot create {}", plan.conf_dir.display()))?;
    for file in &plan.files {
        if let Some(parent) = file.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        write_file(&file.path, &file.contents, file.private)
            .with_context(|| format!("cannot write {}", file.path.display()))?;
    }

    // What was written must load, or `watcher run` would fail later.
    let bootstrap_path = &plan.files[0].path;
    Bootstrap::load(bootstrap_path)?;
    if let Err(diagnostics) = load_dir(&plan.conf_dir) {
        for d in diagnostics {
            eprintln!("{d}");
        }
        bail!("the written configuration has errors");
    }

    if a.test_notification {
        let notifier = Notifier::new(a.smtp.as_ref(), a.public_url.clone())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let contact = Contact {
            name: "admins".into(),
            email: if a.smtp.is_some() {
                a.alert_email.clone()
            } else {
                Vec::new()
            },
            webhook: a.alert_webhook.clone(),
            channels: Default::default(),
            groups: Vec::new(),
            origin: Default::default(),
        };
        for (channel, result) in notifier
            .deliver(
                &contact,
                &Notification::test("admins", a.public_url.clone()),
            )
            .await
        {
            match result {
                Ok(()) => println!("Test {channel}: sent"),
                Err(err) => println!("Test {channel}: failed: {err}"),
            }
        }
    }
    Ok(recovery)
}

/// Turns on two-factor authentication for a new user: the secret sealed
/// with `data_dir/secret.key` (created if missing), and recovery codes.
async fn enable_totp(
    store: &Store,
    data_dir: &Path,
    user_id: i32,
    secret: &[u8],
) -> Result<Vec<String>> {
    let sealer = Sealer::open_or_create(data_dir).map_err(anyhow::Error::msg)?;
    let sealed = auth::seal_totp_secret(&sealer, user_id, secret).map_err(anyhow::Error::msg)?;
    let (codes, hashes) = auth::recovery_codes();
    // The user hasn't logged in yet, so every current code may be used.
    store.enable_totp(user_id, &sealed, 0, &hashes).await?;
    Ok(codes)
}

pub(crate) fn write_file(path: &Path, contents: &str, private: bool) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    if private {
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    if private {
        // An existing file keeps its mode on open; tighten it.
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(contents.as_bytes())
}

/// Checks for this machine: monitoring plugins if installed, else shell.
/// Returns checks.toml, services.toml and hosts.toml.
fn localhost_config() -> (String, String, String) {
    let (checks, services) = match plugin_dir() {
        // Bare names: the plugin directories are check directories.
        Some(_) => (
            "[check.disk]\n\
             command = \"check_disk -w $warn$ -c $crit$ -p $path$\"\n\
             vars = { warn = \"20%\", crit = \"10%\" }\n\
             \n\
             [check.load]\n\
             command = \"check_load -r -w 1.5,1.2,1 -c 3,2.5,2\"\n\
             \n\
             [check.swap]\n\
             command = \"check_swap -w 20% -c 10%\"\n"
                .to_string(),
            &["disk_root", "load", "swap"][..],
        ),
        None => {
            let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
            let disk = r#"df -P $path$ | awk 'NR==2 {sub("%","",$5); print "used=" $5 "%"}'"#;
            let load = r#"echo "load1=$(cut -d' ' -f1 /proc/loadavg) load5=$(cut -d' ' -f2 /proc/loadavg)""#;
            (
                format!(
                    "[check.disk]\nshell = true\ncommand = {}\nthresholds.used = {{ warn = \"> 80\", crit = \"> 90\" }}\n\
                     \n[check.load]\nshell = true\ncommand = {}\nthresholds.load1 = {{ warn = \"> {}\", crit = \"> {}\" }}\n",
                    q(disk),
                    q(load),
                    cpus * 2,
                    cpus * 4
                ),
                &["disk_root", "load"][..],
            )
        }
    };
    let mut objects = String::from("[service.disk_root]\ncheck = \"disk\"\nvars.path = \"/\"\n");
    for s in &services[1..] {
        objects.push_str(&format!("\n[service.{s}]\n"));
    }
    let list: Vec<String> = services.iter().map(|s| q(s)).collect();
    objects.push_str(&format!(
        "\n[servicegroup.linux]\ntitle = \"Linux\"\nservices = [{}]\n",
        list.join(", ")
    ));
    let hosts = format!(
        "[hostgroup.linux]\ntitle = \"Linux Servers\"\nservicegroups = [\"linux\"]\n\
         \n[[host]]\nname = {}\naddress = \"127.0.0.1\"\ngroups = [\"linux\"]\n",
        q(&hostname())
    );
    (checks, objects, hosts)
}

pub(crate) fn plugin_dir() -> Option<PathBuf> {
    watcher_config::check_dirs::PLUGIN_DIRS
        .iter()
        .map(PathBuf::from)
        .find(|d| d.join("check_disk").exists())
}

/// The system's time zone as an IANA name; UTC if it has none.
pub(crate) fn system_timezone() -> String {
    jiff::tz::TimeZone::system()
        .iana_name()
        .unwrap_or("UTC")
        .to_string()
}

pub(crate) fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|h| h.trim().to_string())
        .ok()
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "localhost".into())
}

/// A TOML string literal.
fn q(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

fn tls_str(tls: SmtpTls) -> &'static str {
    match tls {
        SmtpTls::Starttls => "starttls",
        SmtpTls::Tls => "tls",
        SmtpTls::None => "none",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("watcher-init-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn prompts_take_defaults_and_skip() {
        // Role, database, listen, public URL, timezone, SMTP host, alert
        // email, webhook, monitor.
        let input = "\nnone\n\n\n\nnone\nnone\nnone\nn\n";
        let mut reader = input.as_bytes();
        let mut out = Vec::new();
        let mut p = Prompter {
            input: &mut reader,
            output: &mut out,
            tty: false,
        };
        let a = ask(&mut p).await.unwrap();
        assert_eq!(a.role, InstanceRole::Standalone);
        assert_eq!(a.database_url, None);
        assert_eq!(a.listen.as_deref(), Some("127.0.0.1:8080"));
        assert_eq!(a.public_url.as_deref(), Some("http://127.0.0.1:8080"));
        assert_eq!(a.timezone, Some(system_timezone()));
        assert!(a.smtp.is_none() && a.alert_email.is_empty() && !a.monitor_localhost);
        let transcript = String::from_utf8(out).unwrap();
        assert!(
            transcript.contains("Listen address [127.0.0.1:8080]: "),
            "{transcript}"
        );
    }

    #[tokio::test]
    async fn rejects_invalid_answers_and_asks_again() {
        // Unknown role, then default; non-loopback without database, then
        // loopback; unknown time zone, then one that exists.
        let input = "nope\n\nnone\n0.0.0.0:8080\n127.0.0.1:9000\n\nMars/Olympus\nEurope/Berlin\n\
                     none\nnone\nnone\nn\n";
        let mut reader = input.as_bytes();
        let mut out = Vec::new();
        let mut p = Prompter {
            input: &mut reader,
            output: &mut out,
            tty: false,
        };
        let a = ask(&mut p).await.unwrap();
        assert_eq!(a.listen.as_deref(), Some("127.0.0.1:9000"));
        assert_eq!(a.timezone.as_deref(), Some("Europe/Berlin"));
        let transcript = String::from_utf8(out).unwrap();
        assert!(transcript.contains("unknown role: nope"));
        assert!(transcript.contains("unknown time zone \"Mars/Olympus\""));
        assert!(transcript.contains("non-loopback addresses need a database"));
    }

    #[test]
    fn writes_acme_settings() {
        let dir = temp_dir("acme");
        let answers: Answers = toml::from_str(
            r#"
listen = "0.0.0.0:443"
database_url = "postgres://x"
public_url = "https://watcher.example.com"
monitor_localhost = false

[tls.acme]
email = "ops@example.com"
"#,
        )
        .unwrap();
        let plan = Plan::new(&answers, &dir).unwrap();
        let toml = &plan.files[0].contents;
        assert!(
            toml.contains("[tls.acme]\ndomains = [\"watcher.example.com\"]\n"),
            "{toml}"
        );
        assert!(!toml.contains("directory"), "the default CA: {toml}");
        write_file(&plan.files[0].path, toml, true).unwrap();
        let tls = Bootstrap::load(&plan.files[0].path).unwrap().tls.unwrap();
        assert_eq!(tls.acme.unwrap().email.as_deref(), Some("ops@example.com"));

        let err = |text: &str| {
            Plan::new(&toml::from_str(text).unwrap(), &dir)
                .unwrap_err()
                .to_string()
        };
        assert!(err("[tls.acme]\n").contains("set domains"));
        assert!(err("[tls]\nself_signed = true\n[tls.acme]\n").contains("or acme"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn creates_conf_d_without_files_for_it() {
        let dir = temp_dir("empty");
        let answers: Answers = toml::from_str("monitor_localhost = false").unwrap();
        let plan = Plan::new(&answers, &dir).unwrap();
        assert_eq!(plan.files.len(), 1, "only watcher.toml");
        apply(&answers, &plan).await.unwrap();
        assert!(dir.join("conf.d").is_dir());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn writes_a_configuration_that_loads() {
        let dir = temp_dir("plan");
        let answers: Answers = toml::from_str(
            r#"
listen = "127.0.0.1:8080"
public_url = "https://watcher.example.com"
instance_name = "msp-hub"
timezone = "Europe/Berlin"
alert_email = ["ops@example.com"]
alert_webhook = "https://hooks.example.com/w"

[tls]
self_signed = true

[smtp]
host = "mail.example.com"
username = "watcher"
password = "s3cret \"quoted\""
from = "Watcher <watcher@example.com>"
"#,
        )
        .unwrap();
        let plan = Plan::new(&answers, &dir).unwrap();
        let names: Vec<_> = plan
            .files
            .iter()
            .map(|f| f.path.strip_prefix(&dir).unwrap().display().to_string())
            .collect();
        assert_eq!(
            names,
            [
                "watcher.toml",
                "smtp.pass",
                "conf.d/contacts.toml",
                "conf.d/notify.toml",
                "conf.d/checks.toml",
                "conf.d/services.toml",
                "conf.d/hosts.toml"
            ]
        );
        for f in &plan.files {
            std::fs::create_dir_all(f.path.parent().unwrap()).unwrap();
            write_file(&f.path, &f.contents, f.private).unwrap();
        }

        let bootstrap = Bootstrap::load(&dir.join("watcher.toml")).unwrap();
        assert_eq!(bootstrap.instance_name.as_deref(), Some("msp-hub"));
        assert_eq!(bootstrap.timezone.as_deref(), Some("Europe/Berlin"));
        assert!(bootstrap.tls.unwrap().self_signed);
        let smtp = bootstrap.smtp.unwrap();
        assert_eq!(smtp.password_file.unwrap(), dir.join("smtp.pass"));
        assert_eq!(
            std::fs::read_to_string(dir.join("smtp.pass")).unwrap(),
            "s3cret \"quoted\"\n"
        );
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join("watcher.toml"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);

        let loaded = load_dir(&plan.conf_dir).unwrap();
        assert_eq!(loaded.config.contacts[0].email, ["ops@example.com"]);
        assert!(loaded.config.services.len() >= 2);
        let service = &loaded.config.services[0];
        let to = loaded.config.recipients(
            service,
            false,
            watcher_config::Alert::Problem,
            "critical",
            None,
        );
        assert_eq!(to[0].name, "admins");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn example_answers_file_parses() {
        let text = include_str!("../../../examples/init-answers.toml");
        let a: Answers = toml::from_str(text).unwrap();
        assert_eq!(a.admin.unwrap().username, "admin");
        assert_eq!(a.timezone.as_deref(), Some("Europe/Berlin"));
        assert_eq!(a.smtp.unwrap().port(), 587);
    }

    #[test]
    fn validates_answers() {
        let dir = temp_dir("invalid");
        let err = |text: &str| {
            Plan::new(&toml::from_str(text).unwrap(), &dir)
                .unwrap_err()
                .to_string()
        };
        assert!(err(r#"listen = "0.0.0.0:80""#).contains("need database_url"));
        assert!(
            err("[admin]\nusername = \"a\"\npassword = \"long enough pw\"")
                .contains("needs database_url")
        );
        assert!(
            err("database_url = \"postgres://x\"\n[admin]\nusername = \"a\"\npassword = \"short\"")
                .contains("min. 10")
        );
        assert!(err(r#"alert_email = ["nope"]"#).contains("invalid address"));
        assert!(err(r#"public_url = "watcher.example""#).contains("http"));
        assert!(err(r#"role = "node""#).contains("not available"));
        assert!(err("[tls]\ncert = \"/nonexistent.pem\"\nkey = \"/k.pem\"").contains("no file"));
        assert!(err("[tls]\ncert = \"/c.pem\"").contains("cert and key"));
        assert!(err("instance_name = \" \"").contains("empty"));
        assert!(err(r#"timezone = "Mars/Olympus""#).contains("timezone"));
        assert!(
            err(
                "database_url = \"postgres://x\"\n[admin]\nusername = \"a\"\n\
                 password = \"long enough pw\"\ntotp_secret = \"JBSWY3DP\""
            )
            .contains("totp_secret")
        );
        assert!(toml::from_str::<Answers>("typo = 1").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn asks_for_two_factor() {
        let secret = totp::new_secret();
        let ask = |input: String| {
            let mut reader = input.as_bytes();
            let mut out = Vec::new();
            let mut p = Prompter {
                input: &mut reader,
                output: &mut out,
                tty: false,
            };
            let key = ask_totp(&mut p, "admin", &secret).unwrap();
            (key, String::from_utf8(out).unwrap())
        };
        assert_eq!(ask("\n".into()).0, None, "off unless asked for");
        let (key, transcript) = ask("y\nnone\n".into());
        assert_eq!(key, None);
        assert!(transcript.contains("skipped"), "{transcript}");

        // A wrong code, then the one the app shows.
        let code = totp::code(&secret, totp::now());
        let wrong = format!("{:06}", (code.parse::<u32>().unwrap() + 1) % 1_000_000);
        let (key, transcript) = ask(format!("y\n{wrong}\n{code}\n"));
        assert_eq!(key, Some(totp::encode(&secret)));
        assert!(transcript.contains('█'), "the QR code: {transcript}");
        assert!(transcript.contains(&totp::encode(&secret)[..4]));
        assert!(transcript.contains("invalid code"));
    }
}
