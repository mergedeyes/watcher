//! Checks over SSH against an in-process server that runs commands with
//! `sh -c` in a temporary home directory.

use russh::keys::PublicKey;
use russh::server::{self, Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use watcher_core::Termination;
use watcher_exec::ssh::{self, BoxFuture, Credential, Pool, Remote, Target, Vault};
use watcher_exec::{CommandLine, ExecOptions};

#[derive(Clone)]
struct TestServer {
    home: PathBuf,
    /// The key or password it accepts.
    key: PublicKey,
    password: String,
    connections: Arc<AtomicUsize>,
    /// Input for the commands of this connection, by channel.
    inputs: HashMap<ChannelId, mpsc::UnboundedSender<Vec<u8>>>,
}

impl server::Server for TestServer {
    type Handler = Self;

    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
        self.connections.fetch_add(1, Ordering::SeqCst);
        self.clone()
    }
}

impl server::Handler for TestServer {
    type Error = russh::Error;

    async fn auth_publickey(&mut self, _: &str, key: &PublicKey) -> Result<Auth, Self::Error> {
        Ok(match key.key_data() == self.key.key_data() {
            true => Auth::Accept,
            false => Auth::reject(),
        })
    }

    async fn auth_password(&mut self, _: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(match password == self.password {
            true => Auth::Accept,
            false => Auth::reject(),
        })
    }

    async fn channel_open_session(
        &mut self,
        _: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let command = String::from_utf8_lossy(data).into_owned();
        let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
        self.inputs.insert(channel, tx);
        session.channel_success(channel)?;
        let handle = session.handle();
        let home = self.home.clone();
        tokio::spawn(async move {
            let mut child = tokio::process::Command::new("sh")
                .arg("-c")
                .arg(&command)
                .current_dir(&home)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let mut stdin = child.stdin.take().unwrap();
            let feed = async move {
                while let Some(data) = rx.recv().await {
                    if stdin.write_all(&data).await.is_err() {
                        break;
                    }
                }
            };
            let (mut out, mut err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
            let (mut o, mut e) = (Vec::new(), Vec::new());
            let _ = tokio::join!(feed, out.read_to_end(&mut o), err.read_to_end(&mut e));
            let status = child.wait().await.unwrap();
            let _ = handle.data(channel, o).await;
            if !e.is_empty() {
                let _ = handle.extended_data(channel, 1, e).await;
            }
            let code = status.code().unwrap_or(255) as u32;
            let _ = handle.exit_status_request(channel, code).await;
            let _ = handle.eof(channel).await;
            let _ = handle.close(channel).await;
        });
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Some(tx) = self.inputs.get(&channel) {
            let _ = tx.send(data.to_vec());
        }
        Ok(())
    }

    async fn channel_eof(
        &mut self,
        channel: ChannelId,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        self.inputs.remove(&channel);
        Ok(())
    }
}

/// Credentials by name, and host keys pinned on first use.
struct TestVault {
    credentials: HashMap<String, Credential>,
    pinned: Mutex<HashMap<(String, u16), PublicKey>>,
}

impl Vault for TestVault {
    fn credential<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Credential, String>> {
        Box::pin(async move {
            self.credentials
                .get(name)
                .cloned()
                .ok_or_else(|| "no such secret".to_string())
        })
    }

    fn check_host_key<'a>(
        &'a self,
        address: &'a str,
        port: u16,
        key: &'a PublicKey,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let mut pinned = self.pinned.lock().unwrap();
            let known = pinned
                .entry((address.to_string(), port))
                .or_insert_with(|| key.clone());
            match known.key_data() == key.key_data() {
                true => Ok(()),
                false => Err("host key changed".into()),
            }
        })
    }
}

struct Setup {
    pool: Pool,
    vault: Arc<TestVault>,
    port: u16,
    home: PathBuf,
    connections: Arc<AtomicUsize>,
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("watcher-ssh-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

async fn setup(name: &str) -> Setup {
    let home = temp(name);
    let Credential::Key(host_key) = Credential::generate("host").unwrap() else {
        unreachable!()
    };
    let client = Credential::generate("client").unwrap();
    let Credential::Key(client_key) = &client else {
        unreachable!()
    };
    let config = Arc::new(server::Config {
        keys: vec![*host_key],
        auth_rejection_time: Duration::from_millis(10),
        auth_rejection_time_initial: Some(Duration::ZERO),
        ..Default::default()
    });
    let connections = Arc::new(AtomicUsize::new(0));
    let mut server = TestServer {
        home: home.clone(),
        key: client_key.public_key().clone(),
        password: "pw".into(),
        connections: connections.clone(),
        inputs: HashMap::new(),
    };
    let socket = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = socket.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = server.run_on_socket(config, &socket).await;
    });
    let vault = Arc::new(TestVault {
        credentials: HashMap::from([
            ("key".to_string(), client),
            ("pw".to_string(), Credential::Password("pw".into())),
            ("wrong".to_string(), Credential::Password("nope".into())),
        ]),
        pinned: Mutex::default(),
    });
    Setup {
        pool: Pool::new(vault.clone()),
        vault,
        port,
        home,
        connections,
    }
}

impl Setup {
    fn remote(&self, secret: &str) -> Remote {
        Remote {
            target: Target {
                address: "127.0.0.1".into(),
                port: self.port,
                user: "monitor".into(),
                secret: secret.into(),
            },
            copy: false,
        }
    }

    async fn run(&self, remote: &Remote, command: CommandLine) -> watcher_exec::Execution {
        let opts = ExecOptions {
            timeout: Duration::from_secs(10),
            ..Default::default()
        };
        ssh::run(&self.pool, remote, &command, &opts).await
    }
}

fn sh(script: &str) -> CommandLine {
    CommandLine::parse(script, true).unwrap()
}

#[tokio::test]
async fn runs_commands_on_one_connection() {
    let s = setup("run").await;
    let remote = s.remote("key");
    let e = s.run(&remote, sh("echo hi; echo oops >&2; exit 2")).await;
    assert_eq!(e.output.termination, Termination::Exited(2));
    assert_eq!(e.output.stdout, "hi\n");
    assert_eq!(e.output.stderr, "oops\n");

    // Direct commands are quoted for the remote shell.
    let argv = CommandLine::parse("printf '%s|' 'a b' '$HOME'", false).unwrap();
    let e = s.run(&remote, argv).await;
    assert_eq!(e.output.stdout, "a b|$HOME|");

    // More at once than sessions per connection: they wait for each other.
    let s = Arc::new(s);
    let mut runs = tokio::task::JoinSet::new();
    for _ in 0..ssh::MAX_SESSIONS + 4 {
        let (s, remote) = (s.clone(), remote.clone());
        runs.spawn(async move { s.run(&remote, sh("sleep 0.1; echo ok")).await });
    }
    for e in runs.join_all().await {
        assert_eq!(e.output.termination, Termination::Exited(0));
        assert_eq!(e.output.stdout, "ok\n");
    }
    assert_eq!(s.connections.load(Ordering::SeqCst), 1);
    assert_eq!(s.pool.connections(), 1);
}

#[tokio::test]
async fn passwords_and_refused_logins() {
    let s = setup("login").await;
    let e = s.run(&s.remote("pw"), sh("echo pw")).await;
    assert_eq!(e.output.stdout, "pw\n");

    let e = s.run(&s.remote("wrong"), sh("true")).await;
    let Termination::Failed(msg) = e.output.termination else {
        panic!("{:?}", e.output)
    };
    assert!(msg.contains("login refused with secret \"wrong\""), "{msg}");

    let e = s.run(&s.remote("missing"), sh("true")).await;
    assert_eq!(
        e.output.termination,
        Termination::Failed("secret \"missing\": no such secret".into())
    );
}

#[tokio::test]
async fn changed_host_key_is_refused() {
    let s = setup("hostkey").await;
    let Credential::Key(other) = Credential::generate("other").unwrap() else {
        unreachable!()
    };
    s.vault
        .pinned
        .lock()
        .unwrap()
        .insert(("127.0.0.1".into(), s.port), other.public_key().clone());
    let e = s.run(&s.remote("key"), sh("true")).await;
    let Termination::Failed(msg) = e.output.termination else {
        panic!("{:?}", e.output)
    };
    assert!(msg.contains("host key changed"), "{msg}");
}

#[tokio::test]
async fn timeout() {
    let s = setup("timeout").await;
    let opts = ExecOptions {
        timeout: Duration::from_millis(300),
        ..Default::default()
    };
    let e = ssh::run(&s.pool, &s.remote("key"), &sh("sleep 2"), &opts).await;
    assert_eq!(e.output.termination, Termination::TimedOut(opts.timeout));
    assert!(e.duration < Duration::from_secs(2));
}

#[tokio::test]
async fn copies_scripts() {
    let s = setup("copy").await;
    let checks = temp("copy-checks");
    let script = checks.join("check_here");
    let write = |text: &str| {
        std::fs::write(&script, text).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    write("#!/bin/sh\necho \"v1 $1\"\n");
    let remote = Remote {
        copy: true,
        ..s.remote("key")
    };
    let opts = ExecOptions {
        path: Some(checks.as_os_str().into()),
        ..Default::default()
    };
    let command = CommandLine::parse("check_here arg", false).unwrap();
    let e = ssh::run(&s.pool, &remote, &command, &opts).await;
    assert_eq!(e.output.stdout, "v1 arg\n", "{:?}", e.output);

    // A new version replaces the old one.
    write("#!/bin/sh\necho \"v2 $1\"\n");
    let e = ssh::run(&s.pool, &remote, &command, &opts).await;
    assert_eq!(e.output.stdout, "v2 arg\n", "{:?}", e.output);
    let dir = s.home.join(ssh::REMOTE_DIR).join("check_here");
    assert_eq!(files(&dir).len(), 1);

    // Deleted on the host: fails once, then it is uploaded again.
    std::fs::remove_dir_all(&dir).unwrap();
    let e = ssh::run(&s.pool, &remote, &command, &opts).await;
    assert_eq!(e.output.termination, Termination::Exited(127));
    let e = ssh::run(&s.pool, &remote, &command, &opts).await;
    assert_eq!(e.output.stdout, "v2 arg\n", "{:?}", e.output);

    let missing = CommandLine::parse("check_gone", false).unwrap();
    let e = ssh::run(&s.pool, &remote, &missing, &opts).await;
    assert_eq!(
        e.output.termination,
        Termination::Failed("copy: check_gone not found in the check directories".into())
    );
}

fn files(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}
