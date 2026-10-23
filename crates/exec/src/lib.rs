//! Runs check commands.
//!
//! Each check runs in its own process group with stdin closed. On timeout the
//! whole group is killed; when the plugin exits normally, anything it left
//! behind in its group is killed as well. stdout and stderr are captured up
//! to a limit, and the rest is drained and discarded so the plugin never
//! blocks on a full pipe.

pub mod builtin;
pub mod scheduler;

pub use builtin::Builtin;

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use std::ffi::OsStr;
use std::os::unix::process::ExitStatusExt;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use watcher_core::{RawOutput, Termination};

/// Default cap for captured stdout and stderr, each.
pub const DEFAULT_OUTPUT_LIMIT: usize = 64 * 1024;
/// Default check timeout (same as Nagios' `service_check_timeout`).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// How long to keep reading the pipes after the plugin has exited. They only
/// stay open past that if a child escaped the process group (`setsid`).
const DRAIN_GRACE: Duration = Duration::from_millis(500);

/// A command to run, either directly or through `/bin/sh -c`, or a
/// built-in check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandLine {
    Argv(Vec<String>),
    Shell(String),
    Builtin(Builtin),
}

/// Shell-like text for display, e.g. `check_disk -w '20%'`.
impl std::fmt::Display for CommandLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommandLine::Argv(argv) => f.write_str(&shell_words::join(argv)),
            CommandLine::Shell(script) => write!(f, "/bin/sh -c {}", shell_words::quote(script)),
            CommandLine::Builtin(b) => b.fmt(f),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("empty command")]
    Empty,
    #[error("cannot split command: {0}")]
    Split(#[from] shell_words::ParseError),
}

impl CommandLine {
    /// Builds a command from config text. Without `shell`, the text is split
    /// into arguments with shell quoting rules, but no shell is run.
    pub fn parse(command: &str, shell: bool) -> Result<Self, CommandError> {
        if command.trim().is_empty() {
            return Err(CommandError::Empty);
        }
        if shell {
            return Ok(CommandLine::Shell(command.to_string()));
        }
        Ok(CommandLine::Argv(shell_words::split(command)?))
    }

    /// The program that is executed, for messages.
    pub fn program(&self) -> &str {
        match self {
            CommandLine::Argv(argv) => argv.first().map_or("", String::as_str),
            CommandLine::Shell(_) => "/bin/sh",
            CommandLine::Builtin(Builtin::Ping(_)) => "ping (built-in)",
        }
    }

    fn to_command(&self) -> Option<Command> {
        match self {
            CommandLine::Argv(argv) => {
                let (program, args) = argv.split_first()?;
                let mut cmd = Command::new(program);
                cmd.args(args);
                Some(cmd)
            }
            CommandLine::Shell(script) => {
                let mut cmd = Command::new("/bin/sh");
                cmd.arg("-c").arg(script);
                Some(cmd)
            }
            CommandLine::Builtin(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecOptions {
    pub timeout: Duration,
    /// Maximum number of bytes kept from stdout and from stderr.
    pub output_limit: usize,
    /// `PATH` for the check, which is also where its program is looked up;
    /// `None` keeps the server's.
    pub path: Option<Arc<OsStr>>,
}

impl Default for ExecOptions {
    fn default() -> Self {
        ExecOptions {
            timeout: DEFAULT_TIMEOUT,
            output_limit: DEFAULT_OUTPUT_LIMIT,
            path: None,
        }
    }
}

/// The result of running a command.
#[derive(Debug, Clone)]
pub struct Execution {
    pub output: RawOutput,
    pub duration: Duration,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

/// Runs `command` and collects its output. Never fails: problems starting the
/// process are reported as [`Termination::Failed`].
pub async fn run(command: &CommandLine, opts: &ExecOptions) -> Execution {
    let start = Instant::now();
    let deadline = tokio::time::Instant::now() + opts.timeout;
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

    if let CommandLine::Builtin(builtin) = command {
        let (termination, stdout) = match tokio::time::timeout(opts.timeout, builtin.run()).await {
            Ok((code, out)) => (Termination::Exited(code), out),
            Err(_) => (Termination::TimedOut(opts.timeout), String::new()),
        };
        return Execution {
            output: RawOutput {
                termination,
                stdout,
                stderr: String::new(),
            },
            duration: start.elapsed(),
            stdout_truncated: false,
            stderr_truncated: false,
        };
    }
    let Some(mut cmd) = command.to_command() else {
        return failed("empty command".into());
    };
    if let Some(path) = &opts.path {
        cmd.env("PATH", path);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => return failed(format!("{}: {err}", command.program())),
    };
    // The child leads its own process group, so its pid is the group id.
    let pgid = child
        .id()
        .and_then(|id| i32::try_from(id).ok())
        .map(Pid::from_raw);

    let mut stdout = Capture::new(opts.output_limit);
    let mut stderr = Capture::new(opts.output_limit);
    let termination = {
        let (out_pipe, err_pipe) = (child.stdout.take(), child.stderr.take());
        let io = async {
            tokio::join!(drain(out_pipe, &mut stdout), drain(err_pipe, &mut stderr));
        };
        tokio::pin!(io);
        let mut io_done = false;

        // Read the pipes while waiting, or a chatty plugin blocks on write.
        let waited = {
            let wait = tokio::time::timeout_at(deadline, child.wait());
            tokio::pin!(wait);
            loop {
                tokio::select! {
                    () = &mut io, if !io_done => io_done = true,
                    res = &mut wait => break res,
                }
            }
        };

        // On timeout this stops the plugin; otherwise it kills leftover
        // children that would keep the pipes open. There is a tiny window for
        // pid reuse between reaping the leader and this call, as in Nagios.
        if let Some(pgid) = pgid {
            let _ = killpg(pgid, Signal::SIGKILL);
        }
        let termination = match waited {
            Ok(Ok(status)) => termination(status),
            Ok(Err(err)) => {
                Termination::Failed(format!("waiting for {}: {err}", command.program()))
            }
            Err(_) => {
                let _ = child.wait().await;
                Termination::TimedOut(opts.timeout)
            }
        };
        if !io_done {
            let _ = tokio::time::timeout(DRAIN_GRACE, &mut io).await;
        }
        termination
    };

    Execution {
        output: RawOutput {
            termination,
            stdout: stdout.text(),
            stderr: stderr.text(),
        },
        duration: start.elapsed(),
        stdout_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
    }
}

fn termination(status: ExitStatus) -> Termination {
    match (status.code(), status.signal()) {
        (Some(code), _) => Termination::Exited(code),
        (None, Some(signal)) => Termination::Signaled(signal),
        (None, None) => Termination::Failed(format!("unexpected exit status {status}")),
    }
}

/// Keeps the first `limit` bytes of a stream.
struct Capture {
    data: Vec<u8>,
    limit: usize,
    truncated: bool,
}

impl Capture {
    fn new(limit: usize) -> Self {
        Capture {
            data: Vec::new(),
            limit,
            truncated: false,
        }
    }

    fn push(&mut self, chunk: &[u8]) {
        let room = self.limit.saturating_sub(self.data.len());
        if chunk.len() > room {
            self.truncated = true;
        }
        self.data.extend_from_slice(&chunk[..room.min(chunk.len())]);
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.data).into_owned()
    }
}

/// Reads `pipe` to the end, keeping what fits into `capture`.
async fn drain<R: AsyncRead + Unpin>(pipe: Option<R>, capture: &mut Capture) {
    let Some(mut pipe) = pipe else { return };
    let mut buf = [0u8; 8192];
    loop {
        match pipe.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => capture.push(&buf[..n]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_command_lines() {
        assert_eq!(
            CommandLine::parse("check_disk -w '20%' -p /", false).unwrap(),
            CommandLine::Argv(vec![
                "check_disk".into(),
                "-w".into(),
                "20%".into(),
                "-p".into(),
                "/".into()
            ])
        );
        assert_eq!(
            CommandLine::parse("echo $HOME | wc", true).unwrap(),
            CommandLine::Shell("echo $HOME | wc".into())
        );
        assert!(matches!(
            CommandLine::parse("  ", false),
            Err(CommandError::Empty)
        ));
        assert!(matches!(
            CommandLine::parse("echo 'oops", false),
            Err(CommandError::Split(_))
        ));
    }

    #[test]
    fn capture_truncates() {
        let mut c = Capture::new(4);
        c.push(b"ab");
        assert!(!c.truncated);
        c.push(b"cdef");
        assert_eq!(c.text(), "abcd");
        assert!(c.truncated);
    }
}
