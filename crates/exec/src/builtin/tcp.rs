//! `tcp`: connect to a port, optionally send something and look for an
//! answer, like `check_tcp -H <address> -p <port> -s <send> -e <expect>`.

use super::{TimeLimits, Vars, first_line, host_part, label, took, worst};
use std::fmt;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// At most this much of the answer is read when looking for `expect`.
const READ_LIMIT: usize = 64 * 1024;
/// After the first data, the answer is complete when nothing more comes
/// for this long.
const PAUSE: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tcp {
    pub address: String,
    pub port: u16,
    /// Sent after connecting; `\r`, `\n`, `\t` and `\\` are escapes.
    pub send: Option<String>,
    /// Text the answer must contain.
    pub expect: Option<String>,
    pub limits: TimeLimits,
}

impl Tcp {
    /// Vars: `port` (required), `send`, `expect`, `warn`, `crit`.
    pub(super) fn new(address: &str, vars: &Vars<'_>) -> Result<Self, String> {
        Ok(Tcp {
            address: address.to_string(),
            port: vars.port("port", None)?,
            send: vars.get("send").map(unescape),
            expect: vars.get("expect").map(unescape),
            limits: TimeLimits::from_vars(vars)?,
        })
    }

    pub(super) async fn run(&self, wait: Duration) -> (i32, String) {
        let target = format!("{}:{}", host_part(&self.address), self.port);
        let start = Instant::now();
        let stream = match tokio::time::timeout(
            wait,
            TcpStream::connect((self.address.as_str(), self.port)),
        )
        .await
        {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => return (2, format!("TCP CRITICAL - {target}: {e}")),
            Err(_) => {
                return (
                    2,
                    format!(
                        "TCP CRITICAL - {target}: no connection within {}",
                        took(wait)
                    ),
                );
            }
        };
        let connected = start.elapsed();
        let remaining = wait.saturating_sub(connected);
        let answer = match tokio::time::timeout(remaining, self.talk(stream)).await {
            Ok(Ok(answer)) => answer,
            Ok(Err(e)) => return (2, format!("TCP CRITICAL - {target}: {e}")),
            Err(_) => {
                return (
                    2,
                    format!("TCP CRITICAL - {target}: no answer within {}", took(wait)),
                );
            }
        };
        let elapsed = start.elapsed();
        let mut code = self.limits.code(elapsed);
        let mut text = format!("{target} answered in {}", took(elapsed));
        if let Some(answer) = &answer {
            let line = first_line(answer);
            if self
                .expect
                .as_ref()
                .is_some_and(|e| !answer.contains(e.as_str()))
            {
                code = worst(code, 2);
                text = format!("{target}: unexpected answer {line:?}");
            } else if !line.is_empty() {
                text += &format!(": {line}");
            }
        }
        (
            code,
            format!(
                "TCP {} - {text}|{}",
                label(code),
                self.limits.perfdata(elapsed)
            ),
        )
    }

    /// Sends `send`, then reads until `expect` shows up, the connection
    /// closes, or the limit is reached. `None` if nothing is to be read.
    async fn talk(&self, mut stream: TcpStream) -> std::io::Result<Option<String>> {
        if let Some(send) = &self.send {
            stream.write_all(send.as_bytes()).await?;
        }
        let Some(expect) = &self.expect else {
            return Ok(None);
        };
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            // Once something came, a pause means the server waits for us.
            let n = if buf.is_empty() {
                stream.read(&mut chunk).await?
            } else {
                match tokio::time::timeout(PAUSE, stream.read(&mut chunk)).await {
                    Ok(n) => n?,
                    Err(_) => return Ok(Some(String::from_utf8_lossy(&buf).into_owned())),
                }
            };
            buf.extend_from_slice(&chunk[..n]);
            let text = String::from_utf8_lossy(&buf);
            if n == 0 || buf.len() >= READ_LIMIT || text.contains(expect.as_str()) {
                return Ok(Some(text.into_owned()));
            }
        }
    }
}

impl fmt::Display for Tcp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "tcp {}:{}", host_part(&self.address), self.port)?;
        if let Some(s) = &self.send {
            write!(f, " send={s:?}")?;
        }
        if let Some(e) = &self.expect {
            write!(f, " expect={e:?}")?;
        }
        f.write_str(&self.limits.args())
    }
}

/// `\r`, `\n`, `\t` and `\\` in vars, as `check_tcp -E` reads them.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('r') => out.push('\r'),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::{Builtin, vars};
    use tokio::net::TcpListener;

    async fn run(list: &[(&str, &str)]) -> (i32, String) {
        Builtin::new("tcp", "127.0.0.1", &vars(list))
            .unwrap()
            .run(Duration::from_secs(2), None)
            .await
    }

    /// A server that greets with `banner`, then echoes one read.
    async fn server(banner: &'static str) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let _ = s.write_all(banner.as_bytes()).await;
                    let mut buf = [0u8; 256];
                    if let Ok(n) = s.read(&mut buf).await {
                        let _ = s.write_all(&buf[..n]).await;
                    }
                });
            }
        });
        port
    }

    #[test]
    fn settings() {
        assert!(
            Builtin::new("tcp", "h", &vars(&[]))
                .unwrap_err()
                .contains("vars.port")
        );
        let b = Builtin::new(
            "tcp",
            "::1",
            &vars(&[("port", "22"), ("send", "QUIT\\r\\n"), ("warn", "1")]),
        )
        .unwrap();
        assert_eq!(
            b.to_string(),
            "tcp [::1]:22 send=\"QUIT\\r\\n\" warn=1 (built-in)"
        );
        assert_eq!(unescape("a\\nb\\\\c\\x"), "a\nb\\c\\x");
    }

    #[tokio::test]
    async fn connects_and_reads_the_banner() {
        let port = server("SSH-2.0-Test\r\n").await.to_string();
        let (code, out) = run(&[("port", &port)]).await;
        assert_eq!(code, 0, "{out}");
        assert!(
            out.starts_with(&format!("TCP OK - 127.0.0.1:{port} answered in ")),
            "{out}"
        );
        assert!(out.contains("|time=") && out.ends_with(";;;0"), "{out}");

        let (code, out) = run(&[("port", &port), ("expect", "SSH-2.0")]).await;
        assert_eq!(code, 0, "{out}");
        assert!(out.contains(": SSH-2.0-Test|"), "{out}");

        let (code, out) = run(&[("port", &port), ("expect", "SMTP")]).await;
        assert_eq!(code, 2, "{out}");
        assert!(out.contains("unexpected answer \"SSH-2.0-Test\""), "{out}");
    }

    #[tokio::test]
    async fn sends_and_expects() {
        let port = server("").await.to_string();
        let (code, out) = run(&[("port", &port), ("send", "PING\\n"), ("expect", "PING")]).await;
        assert_eq!(code, 0, "{out}");
        // Limits on the response time.
        let (code, out) = run(&[("port", &port), ("warn", "0")]).await;
        assert_eq!(code, 1, "{out}");
        assert!(out.starts_with("TCP WARNING - "), "{out}");
    }

    #[tokio::test]
    async fn refused_and_silent() {
        // A port that was just free.
        let port = {
            let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
            l.local_addr().unwrap().port()
        };
        let (code, out) = run(&[("port", &port.to_string())]).await;
        assert_eq!(code, 2, "{out}");
        assert!(
            out.starts_with(&format!("TCP CRITICAL - 127.0.0.1:{port}: ")),
            "{out}"
        );

        // Accepts, but never answers.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port().to_string();
        tokio::spawn(async move {
            let mut held = Vec::new();
            loop {
                held.push(listener.accept().await.unwrap().0);
            }
        });
        let (code, out) = Builtin::new(
            "tcp",
            "127.0.0.1",
            &vars(&[("port", &port), ("expect", "x")]),
        )
        .unwrap()
        .run(Duration::from_millis(600), None)
        .await;
        assert_eq!(code, 2, "{out}");
        assert!(out.contains("no answer within"), "{out}");
    }
}
