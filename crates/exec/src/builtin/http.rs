//! `http`: request a URL and check the status, the content and the
//! response time, like `check_http` / `check_curl`.

use super::{TimeLimits, Vars, chain, host_part, label, took, worst};
use std::fmt;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// At most this much of the body is read when looking for `content`.
const BODY_LIMIT: usize = 1024 * 1024;
const MAX_REDIRECTS: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Http {
    pub url: String,
    pub method: String,
    /// Request headers, in the order of their names.
    pub headers: Vec<(String, String)>,
    /// The request body.
    pub body: Option<String>,
    /// Basic auth: the user, and the password (may be empty).
    pub user: Option<String>,
    pub password: Option<String>,
    /// Status codes that are OK, as ranges; `None`: 2xx and 3xx OK, 4xx
    /// WARNING, 5xx CRITICAL.
    pub status: Option<Vec<(u16, u16)>>,
    /// Text the body must contain.
    pub content: Option<String>,
    pub follow: bool,
    pub verify: bool,
    pub limits: TimeLimits,
}

impl Http {
    /// Vars: `url` (a URL, or a path on the host; default `/`), `method`,
    /// `headers` (a table), `body`, `user`, `password`, `status`,
    /// `content`, `follow`, `verify`, `warn`, `crit`.
    pub(super) fn new(address: &str, vars: &Vars<'_>) -> Result<Self, String> {
        let url = match vars.get("url").unwrap_or("/") {
            path if path.starts_with('/') => format!("http://{}{path}", host_part(address)),
            url => url.to_string(),
        };
        let parsed = reqwest::Url::parse(&url).map_err(|e| format!("vars.url {url:?}: {e}"))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(format!("vars.url {url:?}: expected http:// or https://"));
        }
        let method = vars.get("method").unwrap_or("GET").to_ascii_uppercase();
        reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|_| format!("vars.method {method:?}: expected GET, HEAD, ..."))?;
        if vars.get("headers").is_some() {
            return Err(
                "vars.headers: expected a table, e.g. { Accept = \"application/json\" }".into(),
            );
        }
        let mut headers = Vec::new();
        for (name, value) in vars.table("headers") {
            reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| format!("vars.headers.{name}: not a valid header name"))?;
            reqwest::header::HeaderValue::from_str(value)
                .map_err(|_| format!("vars.headers.{name}: not a valid header value"))?;
            headers.push((name.to_string(), value.to_string()));
        }
        let user = vars.get("user").map(String::from);
        let password = vars.0.get("password").cloned();
        if user.is_none() && password.as_deref().is_some_and(|p| !p.is_empty()) {
            return Err("vars.password is set without vars.user".into());
        }
        if user.is_some()
            && headers
                .iter()
                .any(|(n, _)| n.eq_ignore_ascii_case("authorization"))
        {
            return Err("set vars.user or an Authorization header, not both".into());
        }
        let status = vars.get("status").map(parse_status).transpose()?;
        Ok(Http {
            url,
            method,
            headers,
            body: vars.0.get("body").filter(|b| !b.is_empty()).cloned(),
            user,
            password: password.filter(|p| !p.is_empty()),
            status,
            content: vars.get("content").map(String::from),
            follow: vars.flag("follow", false)?,
            verify: vars.flag("verify", true)?,
            limits: TimeLimits::from_vars(vars)?,
        })
    }

    fn client(&self) -> Result<&'static reqwest::Client, String> {
        // One client per combination of settings, without idle connections:
        // every run connects anew, as a plugin would.
        static CLIENTS: OnceLock<Result<[reqwest::Client; 4], String>> = OnceLock::new();
        let clients = CLIENTS.get_or_init(|| {
            let build = |verify: bool, follow: bool| {
                reqwest::Client::builder()
                    .user_agent(concat!("Watcher/", env!("CARGO_PKG_VERSION")))
                    .pool_max_idle_per_host(0)
                    .no_proxy()
                    .tls_danger_accept_invalid_certs(!verify)
                    .redirect(if follow {
                        reqwest::redirect::Policy::limited(MAX_REDIRECTS)
                    } else {
                        reqwest::redirect::Policy::none()
                    })
                    .build()
                    .map_err(|e| chain(&e))
            };
            Ok([
                build(true, false)?,
                build(true, true)?,
                build(false, false)?,
                build(false, true)?,
            ])
        });
        let clients = clients.as_ref().map_err(Clone::clone)?;
        Ok(&clients[usize::from(!self.verify) * 2 + usize::from(self.follow)])
    }

    pub(super) async fn run(&self, wait: Duration) -> (i32, String) {
        let client = match self.client() {
            Ok(c) => c,
            Err(e) => return (3, format!("HTTP UNKNOWN - no HTTP client: {e}")),
        };
        let method = reqwest::Method::from_bytes(self.method.as_bytes()).unwrap_or_default();
        let start = Instant::now();
        let mut request = client.request(method, &self.url).timeout(wait);
        for (name, value) in &self.headers {
            request = request.header(name.as_str(), value.as_str());
        }
        if let Some(user) = &self.user {
            request = request.basic_auth(user, self.password.as_ref());
        }
        if let Some(body) = &self.body {
            request = request.body(body.clone());
        }
        let request = request.send();
        let mut response = match request.await {
            Ok(r) => r,
            Err(e) if e.is_timeout() => {
                return (
                    2,
                    format!(
                        "HTTP CRITICAL - {}: no answer within {}",
                        self.url,
                        took(wait)
                    ),
                );
            }
            Err(e) => {
                return (
                    2,
                    format!("HTTP CRITICAL - {}: {}", self.url, error_text(&e)),
                );
            }
        };
        let status = response.status();
        let version = response.version();
        let final_url = response.url().to_string();
        let mut body = Vec::new();
        let mut cut = false;
        if self.method != "HEAD" {
            loop {
                match response.chunk().await {
                    Ok(Some(chunk)) => {
                        body.extend_from_slice(&chunk);
                        if body.len() >= BODY_LIMIT {
                            cut = true;
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(e) if e.is_timeout() => {
                        return (
                            2,
                            format!(
                                "HTTP CRITICAL - {}: body not complete within {}",
                                self.url,
                                took(wait)
                            ),
                        );
                    }
                    Err(e) => {
                        return (
                            2,
                            format!("HTTP CRITICAL - {}: {}", self.url, error_text(&e)),
                        );
                    }
                }
            }
        }
        let elapsed = start.elapsed();
        let size = match response.content_length() {
            Some(n) if cut => n as usize,
            _ => body.len(),
        };

        let code = status.as_u16();
        let mut state = match &self.status {
            Some(ok) => {
                if ok.iter().any(|&(lo, hi)| (lo..=hi).contains(&code)) {
                    0
                } else {
                    2
                }
            }
            None => match code {
                100..=399 => 0,
                400..=499 => 1,
                _ => 2,
            },
        };
        let mut problem =
            (self.status.is_some() && state == 2).then(|| "unexpected status".to_string());
        if let Some(content) = &self.content
            && !String::from_utf8_lossy(&body).contains(content.as_str())
        {
            state = 2;
            problem = Some(format!("content {content:?} not found"));
        }
        state = worst(state, self.limits.code(elapsed));

        let mut text = format!("{version:?} {status}");
        if self.follow && final_url != self.url {
            text += &format!(" from {final_url}");
        }
        text += &format!(", {size} bytes in {}", took(elapsed));
        if let Some(p) = problem {
            text = format!("{p} - {text}");
        }
        (
            state,
            format!(
                "HTTP {} - {text}|{} size={size}B;;;0",
                label(state),
                self.limits.perfdata(elapsed)
            ),
        )
    }
}

impl fmt::Display for Http {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "http {} {}", self.method, self.url)?;
        // Values that look like credentials are left out: the command is
        // shown in the UI and in logs.
        for (name, value) in &self.headers {
            if secret_header(name) {
                write!(f, " header=\"{name}: ***\"")?;
            } else {
                write!(f, " header={:?}", format!("{name}: {value}"))?;
            }
        }
        if let Some(body) = &self.body {
            write!(f, " body=({} bytes)", body.len())?;
        }
        if let Some(user) = &self.user {
            write!(f, " user={user:?}")?;
        }
        if let Some(status) = &self.status {
            let list: Vec<String> = status
                .iter()
                .map(|&(lo, hi)| {
                    if lo == hi {
                        lo.to_string()
                    } else {
                        format!("{lo}-{hi}")
                    }
                })
                .collect();
            write!(f, " status={}", list.join(","))?;
        }
        if let Some(c) = &self.content {
            write!(f, " content={c:?}")?;
        }
        if self.follow {
            f.write_str(" follow")?;
        }
        if !self.verify {
            f.write_str(" verify=false")?;
        }
        f.write_str(&self.limits.args())
    }
}

/// Headers whose values are not shown: `Authorization`, `Cookie`,
/// `X-Api-Key`, `X-Auth-Token`, ...
fn secret_header(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "auth", "cookie", "key", "token", "secret", "password", "session",
    ]
    .iter()
    .any(|word| name.contains(word))
}

/// `200,204,300-399` → ranges.
fn parse_status(text: &str) -> Result<Vec<(u16, u16)>, String> {
    let err = || format!("vars.status {text:?}: expected codes and ranges, e.g. 200,301-302");
    text.split(',')
        .map(|part| {
            let part = part.trim();
            let (lo, hi) = part.split_once('-').unwrap_or((part, part));
            let lo: u16 = lo.trim().parse().map_err(|_| err())?;
            let hi: u16 = hi.trim().parse().map_err(|_| err())?;
            if !(100..=599).contains(&lo) || !(100..=599).contains(&hi) || lo > hi {
                return Err(err());
            }
            Ok((lo, hi))
        })
        .collect()
}

/// What went wrong, without reqwest's wording around it: `connection
/// refused`, `certificate: UnknownIssuer`.
fn error_text(err: &reqwest::Error) -> String {
    let text = chain(err);
    // "error sending request for url (http://...): client error (Connect): ..."
    match text.find("): ") {
        Some(i) if text.starts_with("error sending request") => text[i + 3..].to_string(),
        _ => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::{Builtin, vars};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A server that answers every request with `response`.
    async fn server(response: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let _ = s.read(&mut buf).await;
                    let _ = s.write_all(response.as_bytes()).await;
                });
            }
        });
        format!("127.0.0.1:{port}")
    }

    /// A server that answers with the request it got.
    async fn echo() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut buf = [0u8; 4096];
                    // Headers, then the body by its content-length.
                    loop {
                        let n = s.read(&mut buf).await.unwrap();
                        request.extend_from_slice(&buf[..n]);
                        let text = String::from_utf8_lossy(&request).to_string();
                        if let Some(end) = text.find("\r\n\r\n") {
                            let length = text
                                .lines()
                                .find_map(|l| l.strip_prefix("content-length: "))
                                .map_or(0, |n| n.trim().parse().unwrap());
                            if request.len() >= end + 4 + length || n == 0 {
                                break;
                            }
                        }
                    }
                    let head = format!(
                        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        request.len()
                    );
                    let _ = s.write_all(head.as_bytes()).await;
                    let _ = s.write_all(&request).await;
                });
            }
        });
        format!("127.0.0.1:{port}")
    }

    async fn run(address: &str, list: &[(&str, &str)]) -> (i32, String) {
        let _ = rustls::crypto::ring::default_provider().install_default();
        Builtin::new("http", address, &vars(list))
            .unwrap()
            .run(Duration::from_secs(3), None)
            .await
    }

    const OK: &str =
        "HTTP/1.1 200 OK\r\ncontent-length: 11\r\nconnection: close\r\n\r\nhello world";

    #[test]
    fn settings() {
        let b = Builtin::new("http", "10.0.0.5", &vars(&[])).unwrap();
        assert_eq!(b.to_string(), "http GET http://10.0.0.5/ (built-in)");
        let b = Builtin::new(
            "http",
            "::1",
            &vars(&[
                ("url", "/health"),
                ("status", "200, 300-302"),
                ("verify", "no"),
                ("crit", "2s"),
            ]),
        )
        .unwrap();
        assert_eq!(
            b.to_string(),
            "http GET http://[::1]/health status=200,300-302 verify=false crit=2 (built-in)"
        );
        let b = Builtin::new(
            "http",
            "h",
            &vars(&[
                ("method", "post"),
                ("headers.Content-Type", "application/json"),
                ("headers.X-Api-Key", "s3cret"),
                ("body", "{}"),
                ("user", "admin"),
                ("password", "pw"),
            ]),
        )
        .unwrap();
        let shown = b.to_string();
        assert_eq!(
            shown,
            "http POST http://h/ header=\"Content-Type: application/json\" \
             header=\"X-Api-Key: ***\" body=(2 bytes) user=\"admin\" (built-in)"
        );
        assert!(!shown.contains("pw"));
        let err = |list: &[(&str, &str)]| Builtin::new("http", "h", &vars(list)).unwrap_err();
        assert!(err(&[("headers", "Accept: */*")]).contains("expected a table"));
        assert!(err(&[("headers.Bad Name", "x")]).contains("vars.headers.Bad Name"));
        assert!(err(&[("headers.X", "a\nb")]).contains("not a valid header value"));
        assert!(err(&[("password", "pw")]).contains("without vars.user"));
        assert!(err(&[("user", "a"), ("headers.authorization", "Bearer x")]).contains("not both"));
        assert!(Builtin::new("http", "h", &vars(&[("url", "ftp://h/")])).is_err());
        assert!(Builtin::new("http", "h", &vars(&[("status", "2xx")])).is_err());
        assert!(Builtin::new("http", "h", &vars(&[("status", "302-200")])).is_err());
    }

    #[tokio::test]
    async fn status_content_and_time() {
        let addr = server(OK).await;
        let (code, out) = run(&addr, &[("url", "/x")]).await;
        assert_eq!(code, 0, "{out}");
        assert!(
            out.starts_with("HTTP OK - HTTP/1.1 200 OK, 11 bytes in "),
            "{out}"
        );
        assert!(
            out.contains("|time=") && out.ends_with(" size=11B;;;0"),
            "{out}"
        );

        let (code, out) = run(&addr, &[("content", "world")]).await;
        assert_eq!(code, 0, "{out}");
        let (code, out) = run(&addr, &[("content", "moon")]).await;
        assert_eq!(code, 2, "{out}");
        assert!(
            out.starts_with("HTTP CRITICAL - content \"moon\" not found - HTTP/1.1 200 OK"),
            "{out}"
        );

        let (code, out) = run(&addr, &[("status", "204")]).await;
        assert_eq!(code, 2, "{out}");
        assert!(
            out.starts_with("HTTP CRITICAL - unexpected status - "),
            "{out}"
        );

        let (code, out) = run(&addr, &[("warn", "0")]).await;
        assert_eq!(code, 1, "{out}");
    }

    #[tokio::test]
    async fn headers_auth_and_body() {
        let addr = echo().await;
        let base = [
            ("method", "PUT"),
            ("headers.X-Test", "$1"),
            ("headers.User-Agent", "probe"),
            ("body", "{\"a\": 1}"),
        ];
        for expect in [
            "PUT / HTTP/1.1",
            "x-test: $1",
            "user-agent: probe",
            "content-length: 8",
            "\r\n\r\n{\"a\": 1}",
        ] {
            let mut list = base.to_vec();
            list.push(("content", expect));
            let (code, out) = run(&addr, &list).await;
            assert_eq!(code, 0, "{expect:?}: {out}");
        }
        let (code, out) = run(&addr, &[("content", "Watcher/")]).await;
        assert_eq!(code, 0, "{out}");
        // admin:pw
        let auth = [
            ("user", "admin"),
            ("password", "pw"),
            ("content", "authorization: Basic YWRtaW46cHc="),
        ];
        let (code, out) = run(&addr, &auth).await;
        assert_eq!(code, 0, "{out}");
    }

    #[tokio::test]
    async fn default_status_mapping() {
        let missing = server("HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n").await;
        assert_eq!(run(&missing, &[]).await.0, 1);
        assert_eq!(run(&missing, &[("status", "404")]).await.0, 0);
        let broken = server("HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\n\r\n").await;
        assert_eq!(run(&broken, &[]).await.0, 2);
        let moved = server("HTTP/1.1 301 Moved Permanently\r\nlocation: http://127.0.0.1:1/\r\ncontent-length: 0\r\n\r\n").await;
        let (code, out) = run(&moved, &[]).await;
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("301 Moved Permanently"), "{out}");
        // Followed: the target refuses.
        let (code, out) = run(&moved, &[("follow", "true")]).await;
        assert_eq!(code, 2, "{out}");
    }

    #[tokio::test]
    async fn refused_and_slow() {
        let port = {
            let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
            l.local_addr().unwrap().port()
        };
        let (code, out) = run(&format!("127.0.0.1:{port}"), &[]).await;
        assert_eq!(code, 2, "{out}");
        assert!(
            out.starts_with(&format!("HTTP CRITICAL - http://127.0.0.1:{port}/: ")),
            "{out}"
        );

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let mut held = Vec::new();
            loop {
                held.push(listener.accept().await.unwrap().0);
            }
        });
        let _ = rustls::crypto::ring::default_provider().install_default();
        let (code, out) = Builtin::new("http", &addr, &vars(&[]))
            .unwrap()
            .run(Duration::from_millis(600), None)
            .await;
        assert_eq!(code, 2, "{out}");
        assert!(out.contains("no answer within"), "{out}");
    }
}
