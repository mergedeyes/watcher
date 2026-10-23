//! Checks on remote hosts, over SSH.
//!
//! One connection per target (address, port, user, secret) is kept open and
//! shared: each check opens a session channel on it, at most
//! [`MAX_SESSIONS`] at a time (OpenSSH allows 10 by default). Commands run
//! with `sh`, whatever the login shell is (sh, bash, ksh, zsh, fish; csh
//! only one-line commands), with the usual plugin directories added to
//! `PATH`. Host keys are pinned on first use, see
//! [`Vault::check_host_key`]. Connections idle for [`IDLE`] are closed.
//!
//! With `copy`, the program is first uploaded from the check directories
//! to `.watcher/checks/<name>/<hash>` in the remote user's home and run
//! from there; older versions are removed.

pub mod secret;

pub use russh::keys::PublicKey;
pub use secret::{Credential, Kind, Sealer};

use crate::{Capture, CommandLine, ExecOptions, Execution};
use russh::client::{self, Handle};
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::{ChannelMsg, Sig};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fmt;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::time::{Instant, timeout_at};
use watcher_core::{RawOutput, Termination};

/// Session channels per connection at a time.
pub const MAX_SESSIONS: usize = 8;
/// Connections unused this long are closed.
pub const IDLE: Duration = Duration::from_secs(300);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// A failed connection is retried after this long; until then checks fail
/// with the same error right away.
const RETRY_AFTER: Duration = Duration::from_secs(10);
/// Added to the remote `PATH`, as locally (see `CheckDirs`).
const PLUGIN_DIRS: &str = "/usr/lib/monitoring-plugins:/usr/lib/nagios/plugins:\
    /usr/lib64/nagios/plugins:/usr/local/libexec/monitoring-plugins:/usr/local/nagios/libexec";
/// Where copied programs go, relative to the remote user's home.
pub const REMOTE_DIR: &str = ".watcher/checks";

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Credentials and host keys, kept by the caller (in the database).
pub trait Vault: Send + Sync {
    /// The credential stored as `name`.
    fn credential<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Credential, String>>;

    /// Whether `key` may be trusted for `address:port`: the first key seen
    /// is pinned; a different one is refused until it is accepted.
    fn check_host_key<'a>(
        &'a self,
        address: &'a str,
        port: u16,
        key: &'a PublicKey,
    ) -> BoxFuture<'a, Result<(), String>>;
}

/// Where and as whom to log in.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Target {
    pub address: String,
    pub port: u16,
    pub user: String,
    /// The credential's name in the [`Vault`].
    pub secret: String,
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.port {
            22 => write!(f, "{}@{}", self.user, self.address),
            port => write!(f, "{}@{}:{port}", self.user, self.address),
        }
    }
}

/// How a job runs on its host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub target: Target,
    /// Upload the program first.
    pub copy: bool,
}

/// Open connections, by target.
pub struct Pool {
    vault: Option<Arc<dyn Vault>>,
    /// Why there is no vault.
    unavailable: String,
    config: Arc<client::Config>,
    entries: Mutex<HashMap<Target, Arc<Entry>>>,
    /// Programs known to be on a target: (target, remote path).
    uploaded: Mutex<HashSet<(Target, String)>>,
}

struct Entry {
    slot: tokio::sync::Mutex<Slot>,
    used: Mutex<Instant>,
}

enum Slot {
    Empty,
    Up(Arc<Conn>),
    Failed(Instant, String),
}

struct Conn {
    handle: Handle<Client>,
    sessions: Arc<Semaphore>,
}

impl Pool {
    pub fn new(vault: Arc<dyn Vault>) -> Self {
        Self::with(Some(vault), String::new())
    }

    /// A pool whose checks fail with `reason`, e.g. without a database.
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self::with(None, reason.into())
    }

    fn with(vault: Option<Arc<dyn Vault>>, unavailable: String) -> Self {
        let config = client::Config {
            keepalive_interval: Some(Duration::from_secs(30)),
            keepalive_max: 3,
            nodelay: true,
            ..Default::default()
        };
        Pool {
            vault,
            unavailable,
            config: Arc::new(config),
            entries: Mutex::default(),
            uploaded: Mutex::default(),
        }
    }

    /// Open connections.
    pub fn connections(&self) -> usize {
        let entries = self.entries.lock().expect("not poisoned");
        entries
            .values()
            .filter(|e| {
                e.slot
                    .try_lock()
                    .is_ok_and(|s| matches!(&*s, Slot::Up(c) if !c.handle.is_closed()))
            })
            .count()
    }

    /// Lets failed connections be retried right away, e.g. after a host
    /// key was accepted or a secret changed.
    pub fn reset_failed(&self) {
        let entries = self.entries.lock().expect("not poisoned");
        for entry in entries.values() {
            if let Ok(mut slot) = entry.slot.try_lock()
                && matches!(&*slot, Slot::Failed(..))
            {
                *slot = Slot::Empty;
            }
        }
    }

    /// The entry for `target`; closes idle connections on the way.
    fn entry(&self, target: &Target) -> Arc<Entry> {
        let now = Instant::now();
        let mut entries = self.entries.lock().expect("not poisoned");
        entries.retain(|t, e| {
            t == target
                || Arc::strong_count(e) > 1
                || now.duration_since(*e.used.lock().expect("not poisoned")) < IDLE
        });
        let entry = entries.entry(target.clone()).or_insert_with(|| {
            Arc::new(Entry {
                slot: tokio::sync::Mutex::new(Slot::Empty),
                used: Mutex::new(now),
            })
        });
        *entry.used.lock().expect("not poisoned") = now;
        entry.clone()
    }

    /// An open connection to `target`, connecting if needed.
    async fn connection(&self, target: &Target, deadline: Instant) -> Result<Arc<Conn>, String> {
        let entry = self.entry(target);
        let mut slot = timeout_at(deadline, entry.slot.lock())
            .await
            .map_err(|_| format!("{target}: timed out waiting for the connection"))?;
        match &*slot {
            Slot::Up(conn) if !conn.handle.is_closed() => return Ok(conn.clone()),
            Slot::Failed(at, error) if at.elapsed() < RETRY_AFTER => return Err(error.clone()),
            _ => {}
        }
        let until = deadline.min(Instant::now() + CONNECT_TIMEOUT);
        let result = match timeout_at(until, self.connect(target)).await {
            Ok(result) => result,
            Err(_) => Err(format!("{target}: connection timed out")),
        };
        match result {
            Ok(conn) => {
                let conn = Arc::new(conn);
                *slot = Slot::Up(conn.clone());
                Ok(conn)
            }
            Err(error) => {
                *slot = Slot::Failed(Instant::now(), error.clone());
                Err(error)
            }
        }
    }

    /// Forgets `conn` if it is still the one for `target`.
    async fn discard(&self, target: &Target, conn: &Arc<Conn>) {
        let entry = self.entry(target);
        let mut slot = entry.slot.lock().await;
        if matches!(&*slot, Slot::Up(c) if Arc::ptr_eq(c, conn)) {
            *slot = Slot::Empty;
        }
    }

    async fn connect(&self, target: &Target) -> Result<Conn, String> {
        let vault = self.vault.clone().ok_or_else(|| self.unavailable.clone())?;
        let credential = vault
            .credential(&target.secret)
            .await
            .map_err(|e| format!("secret {:?}: {e}", target.secret))?;
        let client = Client {
            vault,
            address: target.address.clone(),
            port: target.port,
        };
        let addr = (target.address.as_str(), target.port);
        let mut handle = client::connect(self.config.clone(), addr, client)
            .await
            .map_err(|e| format!("{target}: {e}"))?;
        let auth = match credential {
            Credential::Key(key) => {
                // The hash only matters for RSA, which isn't supported.
                let key = PrivateKeyWithHashAlg::new(Arc::new(*key), None);
                handle.authenticate_publickey(&target.user, key).await
            }
            Credential::Password(password) => {
                handle.authenticate_password(&target.user, password).await
            }
        }
        .map_err(|e| format!("{target}: {e}"))?;
        if !auth.success() {
            return Err(format!(
                "{target}: login refused with secret {:?}",
                target.secret
            ));
        }
        Ok(Conn {
            handle,
            sessions: Arc::new(Semaphore::new(MAX_SESSIONS)),
        })
    }

    /// Runs `command` on `target` with `stdin` as its input. Opening a
    /// session on a connection that went away reconnects once.
    async fn exec(
        &self,
        target: &Target,
        command: &str,
        stdin: Option<&[u8]>,
        opts: &ExecOptions,
        deadline: Instant,
    ) -> Result<Ran, String> {
        let mut retried = false;
        loop {
            let conn = self.connection(target, deadline).await?;
            let _permit = timeout_at(deadline, conn.sessions.clone().acquire_owned())
                .await
                .map_err(|_| format!("{target}: timed out waiting for a free session"))?
                .map_err(|_| format!("{target}: connection closed"))?;
            let channel = match timeout_at(deadline, conn.handle.channel_open_session()).await {
                Ok(Ok(channel)) => channel,
                Ok(Err(err)) => {
                    self.discard(target, &conn).await;
                    if !retried {
                        retried = true;
                        continue;
                    }
                    return Err(format!("{target}: {err}"));
                }
                Err(_) => return Err(format!("{target}: timed out opening a session")),
            };
            return Ok(session(channel, command, stdin, opts, deadline).await);
        }
    }

    /// Uploads the program of `argv` to `target` unless it is there, and
    /// returns `argv` with the remote path as program, and that path.
    async fn upload(
        &self,
        target: &Target,
        argv: &[String],
        opts: &ExecOptions,
        deadline: Instant,
    ) -> Result<(Vec<String>, String), String> {
        let program = argv.first().ok_or("empty command")?;
        let local = find(program, opts.path.as_deref())
            .ok_or_else(|| format!("copy: {program} not found in the check directories"))?;
        let read = local.clone();
        let bytes = tokio::task::spawn_blocking(move || std::fs::read(read))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("copy: {}: {e}", local.display()))?;
        let hash: String = Sha256::digest(&bytes)[..8]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let name = local
            .file_name()
            .map_or_else(|| program.clone(), |n| n.to_string_lossy().into_owned());
        let dir = format!("{REMOTE_DIR}/{name}");
        let path = format!("{dir}/{hash}");
        let key = (target.clone(), path.clone());
        if !self.uploaded.lock().expect("not poisoned").contains(&key) {
            let d = shell_words::quote(&dir);
            let script = format!(
                "mkdir -p {d} && cat > {d}/.upload.$$ && chmod 755 {d}/.upload.$$ \
                 && mv -f {d}/.upload.$$ {p} && cd {d} \
                 && for f in *; do [ \"$f\" = {hash} ] || rm -f \"$f\"; done",
                p = shell_words::quote(&path),
            );
            let ran = self
                .exec(target, &sh_c(&script), Some(&bytes), opts, deadline)
                .await?;
            if ran.output.termination != Termination::Exited(0) {
                let why = match ran.output.stderr.trim() {
                    "" => ran.output.termination.to_string(),
                    err => err.to_string(),
                };
                return Err(format!("copy of {name} to {target} failed: {why}"));
            }
            self.uploaded.lock().expect("not poisoned").insert(key);
        }
        let mut argv = argv.to_vec();
        argv[0] = path.clone();
        Ok((argv, path))
    }
}

/// Runs `command` on the remote's host. Never fails: connection problems
/// are reported as [`Termination::Failed`].
pub async fn run(
    pool: &Pool,
    remote: &Remote,
    command: &CommandLine,
    opts: &ExecOptions,
) -> Execution {
    let start = std::time::Instant::now();
    let deadline = Instant::now() + opts.timeout;
    let target = &remote.target;
    let failed = |msg: String| Execution {
        output: RawOutput {
            termination: Termination::Failed(msg),
            stdout: String::new(),
            stderr: String::new(),
        },
        duration: start.elapsed(),
        stdout_truncated: false,
        stderr_truncated: false,
    };
    let mut copied = None;
    let line = match command {
        CommandLine::Argv(argv) if remote.copy => {
            match pool.upload(target, argv, opts, deadline).await {
                Ok((argv, path)) => {
                    copied = Some(path);
                    shell_words::join(argv)
                }
                Err(msg) => return failed(msg),
            }
        }
        CommandLine::Argv(argv) => shell_words::join(argv),
        CommandLine::Shell(script) => script.clone(),
        CommandLine::Builtin(_) => return failed("built-in checks run locally".into()),
    };
    let line = sh_c(&format!(
        "PATH=\"$PATH:{PLUGIN_DIRS}\"; export PATH; {line}"
    ));
    let ran = match pool.exec(target, &line, None, opts, deadline).await {
        Ok(ran) => ran,
        Err(msg) => return failed(msg),
    };
    // Deleted on the host since: upload it again next time.
    if let Some(path) = copied
        && matches!(ran.output.termination, Termination::Exited(126 | 127))
    {
        let key = (target.clone(), path);
        pool.uploaded.lock().expect("not poisoned").remove(&key);
    }
    let output = match ran.output.termination {
        Termination::TimedOut(_) => RawOutput {
            termination: Termination::TimedOut(opts.timeout),
            ..ran.output
        },
        _ => ran.output,
    };
    Execution {
        output,
        duration: start.elapsed(),
        stdout_truncated: ran.stdout_truncated,
        stderr_truncated: ran.stderr_truncated,
    }
}

/// What a session produced.
struct Ran {
    output: RawOutput,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

/// Runs `command` in a session channel until it exits or `deadline`.
async fn session(
    mut channel: russh::Channel<client::Msg>,
    command: &str,
    stdin: Option<&[u8]>,
    opts: &ExecOptions,
    deadline: Instant,
) -> Ran {
    let mut stdout = Capture::new(opts.output_limit);
    let mut stderr = Capture::new(opts.output_limit);
    let io = async {
        channel.exec(true, command).await?;
        if let Some(data) = stdin {
            channel.data(data).await?;
        }
        // No input, as for local checks (stdin is /dev/null there).
        channel.eof().await?;
        let mut status = None;
        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { data } => stdout.push(&data),
                ChannelMsg::ExtendedData { data, ext: 1 } => stderr.push(&data),
                ChannelMsg::ExitStatus { exit_status } => {
                    status = Some(Termination::Exited(exit_status as i32));
                }
                ChannelMsg::ExitSignal { signal_name, .. } => status = Some(signaled(signal_name)),
                ChannelMsg::Failure => {
                    status = Some(Termination::Failed("the host refused the command".into()));
                    break;
                }
                ChannelMsg::Close => break,
                _ => {}
            }
        }
        Ok::<_, russh::Error>(status)
    };
    let termination = match timeout_at(deadline, io).await {
        Ok(Ok(Some(termination))) => termination,
        Ok(Ok(None)) => Termination::Failed("the session ended without an exit status".into()),
        Ok(Err(err)) => Termination::Failed(err.to_string()),
        Err(_) => {
            // OpenSSH ignores signals from clients; closing the channel
            // hangs up the command's terminal-less session.
            let _ = channel.signal(Sig::KILL).await;
            let _ = channel.close().await;
            Termination::TimedOut(opts.timeout)
        }
    };
    Ran {
        output: RawOutput {
            termination,
            stdout: stdout.text(),
            stderr: stderr.text(),
        },
        stdout_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
    }
}

fn signaled(sig: Sig) -> Termination {
    use nix::sys::signal::Signal as S;
    let signal = match sig {
        Sig::ABRT => S::SIGABRT,
        Sig::ALRM => S::SIGALRM,
        Sig::FPE => S::SIGFPE,
        Sig::HUP => S::SIGHUP,
        Sig::ILL => S::SIGILL,
        Sig::INT => S::SIGINT,
        Sig::KILL => S::SIGKILL,
        Sig::PIPE => S::SIGPIPE,
        Sig::QUIT => S::SIGQUIT,
        Sig::SEGV => S::SIGSEGV,
        Sig::TERM => S::SIGTERM,
        Sig::USR1 => S::SIGUSR1,
        Sig::Custom(name) => return Termination::Failed(format!("killed by SIG{name}")),
    };
    Termination::Signaled(signal as i32)
}

/// `SHA256:...` of a public key line (`ssh-ed25519 AAAA...`).
pub fn fingerprint(public_key: &str) -> Option<String> {
    let key = PublicKey::from_openssh(public_key).ok()?;
    Some(key.fingerprint(HashAlg::Sha256).to_string())
}

/// A public key as a line, without comment.
pub fn key_line(key: &PublicKey) -> String {
    let mut key = key.clone();
    key.set_comment("");
    key.to_openssh().unwrap_or_default().trim().to_string()
}

/// `exec sh -c '<script>'` for the login shell. Quotes and backslashes go
/// outside the single quotes, escaped, which POSIX shells and fish read
/// alike (fish takes `\\` and `\'` in single quotes as escapes).
fn sh_c(script: &str) -> String {
    let mut out = String::from("exec sh -c '");
    for c in script.chars() {
        match c {
            '\'' => out.push_str(r"'\''"),
            '\\' => out.push_str(r"'\\'"),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// `program` as a path, or looked up in `path` (the server's `PATH` if
/// `None`).
fn find(program: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    if program.contains('/') {
        return Some(PathBuf::from(program));
    }
    let env = std::env::var_os("PATH");
    let path = path.or(env.as_deref())?;
    std::env::split_paths(path)
        .map(|dir| dir.join(program))
        .find(|p| p.is_file())
}

/// Checks the server's host key with the vault.
struct Client {
    vault: Arc<dyn Vault>,
    address: String,
    port: u16,
}

#[derive(Debug)]
enum ClientError {
    Ssh(russh::Error),
    HostKey(String),
}

impl From<russh::Error> for ClientError {
    fn from(err: russh::Error) -> Self {
        ClientError::Ssh(err)
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::Ssh(err) => err.fmt(f),
            ClientError::HostKey(msg) => f.write_str(msg),
        }
    }
}

impl client::Handler for Client {
    type Error = ClientError;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => self
                .vault
                .check_host_key(&self.address, self.port, key)
                .await
                .map(|()| true)
                .map_err(ClientError::HostKey),
            PublicKeyOrCertificate::Certificate(_) => Err(ClientError::HostKey(
                "host certificates are not supported".into(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets() {
        let mut t = Target {
            address: "db1".into(),
            port: 22,
            user: "monitor".into(),
            secret: "monitor".into(),
        };
        assert_eq!(t.to_string(), "monitor@db1");
        t.port = 2222;
        assert_eq!(t.to_string(), "monitor@db1:2222");
    }

    #[test]
    fn key_lines() {
        let Credential::Key(key) = Credential::generate("comment").unwrap() else {
            unreachable!()
        };
        let line = key_line(key.public_key());
        assert!(line.starts_with("ssh-ed25519 ") && !line.contains("comment"));
        let fp = fingerprint(&line).unwrap();
        assert_eq!(fp, key.fingerprint(HashAlg::Sha256).to_string());
        assert!(fp.starts_with("SHA256:"));
        assert_eq!(fingerprint("nonsense"), None);
    }

    #[test]
    fn quoting_for_login_shells() {
        let script = r#"printf '%s|' "a'b" 'c\\d' "$((1+2))" "$0"
echo done"#;
        let expected = "a'b|c\\\\d|3|sh|done\n";
        // sshd runs commands as `$SHELL -c <command>`.
        for shell in ["sh", "bash", "zsh", "fish"] {
            let Ok(out) = std::process::Command::new(shell)
                .arg("-c")
                .arg(sh_c(script))
                .output()
            else {
                continue;
            };
            assert_eq!(String::from_utf8_lossy(&out.stdout), expected, "{shell}");
        }
    }

    #[test]
    fn signals() {
        assert_eq!(signaled(Sig::KILL), Termination::Signaled(9));
        assert_eq!(
            signaled(Sig::Custom("XCPU".into())),
            Termination::Failed("killed by SIGXCPU".into())
        );
    }
}
