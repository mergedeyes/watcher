//! Checks that run in-process instead of as a plugin: `ping`, `tcp`,
//! `http`, `dns` and `tls`.
//!
//! They print plugin output (Nagios format with perfdata) and an exit code,
//! so their results go through the same evaluation as any plugin's. Their
//! settings come from the service's vars, with the names the matching
//! Nagios plugins use where there is one.

mod dns;
mod http;
mod ping;
mod tcp;
mod tls;

pub use dns::Dns;
pub use http::Http;
pub use ping::{Limits, Ping};
pub use tcp::Tcp;
pub use tls::Tls;

use std::collections::BTreeMap;
use std::fmt;
use std::net::IpAddr;
use std::time::Duration;

/// The built-in checks, by name.
pub const NAMES: [&str; 5] = ["ping", "tcp", "http", "dns", "tls"];

/// The vars built-in check `name` reads.
pub fn vars_of(name: &str) -> &'static [&'static str] {
    match name {
        "ping" => &["address", "packets", "warn", "crit"],
        "tcp" => &["address", "port", "send", "expect", "warn", "crit"],
        "http" => &[
            "address", "url", "method", "headers", "body", "user", "password", "status", "content",
            "follow", "verify", "warn", "crit",
        ],
        "dns" => &[
            "address", "name", "type", "server", "port", "expect", "warn", "crit",
        ],
        "tls" => &["address", "port", "name", "warn", "crit", "verify"],
        _ => &[],
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Builtin {
    Ping(Ping),
    Tcp(Tcp),
    Http(Box<Http>),
    Dns(Dns),
    Tls(Tls),
}

impl Builtin {
    /// Builds built-in check `name` for a host at `address`, with the
    /// settings in `vars`. The var `address` replaces the host's.
    pub fn new(name: &str, address: &str, vars: &BTreeMap<String, String>) -> Result<Self, String> {
        let vars = Vars(vars);
        let address = vars.get("address").unwrap_or(address);
        Ok(match name {
            "ping" => Builtin::Ping(Ping::new(address, &vars)?),
            "tcp" => Builtin::Tcp(Tcp::new(address, &vars)?),
            "http" => Builtin::Http(Box::new(Http::new(address, &vars)?)),
            "dns" => Builtin::Dns(Dns::new(address, &vars)?),
            "tls" => Builtin::Tls(Tls::new(address, &vars)?),
            _ => {
                return Err(format!(
                    "unknown builtin {name:?} (known: {})",
                    NAMES.join(", ")
                ));
            }
        })
    }

    /// The check's name: `http`.
    pub fn name(&self) -> &'static str {
        match self {
            Builtin::Ping(_) => "ping",
            Builtin::Tcp(_) => "tcp",
            Builtin::Http(_) => "http",
            Builtin::Dns(_) => "dns",
            Builtin::Tls(_) => "tls",
        }
    }

    /// For messages: `http (built-in)`.
    pub fn program(&self) -> &'static str {
        match self {
            Builtin::Ping(_) => "ping (built-in)",
            Builtin::Tcp(_) => "tcp (built-in)",
            Builtin::Http(_) => "http (built-in)",
            Builtin::Dns(_) => "dns (built-in)",
            Builtin::Tls(_) => "tls (built-in)",
        }
    }

    /// Runs the check: the exit code and plugin output. Network waits end
    /// shortly before `timeout`, so a slow answer is reported as such
    /// instead of the run being cut off.
    pub async fn run(&self, timeout: Duration) -> (i32, String) {
        let wait = timeout
            .saturating_sub(Duration::from_millis(200))
            .max(Duration::from_millis(100));
        match self {
            Builtin::Ping(p) => p.run().await,
            Builtin::Tcp(c) => c.run(wait).await,
            Builtin::Http(c) => c.run(wait).await,
            Builtin::Dns(c) => c.run(wait).await,
            Builtin::Tls(c) => c.run(wait).await,
        }
    }
}

impl fmt::Display for Builtin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Builtin::Ping(c) => c.fmt(f)?,
            Builtin::Tcp(c) => c.fmt(f)?,
            Builtin::Http(c) => c.fmt(f)?,
            Builtin::Dns(c) => c.fmt(f)?,
            Builtin::Tls(c) => c.fmt(f)?,
        }
        f.write_str(" (built-in)")
    }
}

/// A check's vars, with parsing and messages that name the var.
pub(crate) struct Vars<'a>(&'a BTreeMap<String, String>);

impl Vars<'_> {
    /// A var that is set and not blank.
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(|v| v.trim()).filter(|v| !v.is_empty())
    }

    /// The entries of a table var like `headers = { Accept = "..." }`,
    /// which arrive as `headers.Accept`. Values are not trimmed.
    fn table<'k>(&'k self, key: &'k str) -> impl Iterator<Item = (&'k str, &'k str)> + 'k {
        self.0.iter().filter_map(move |(k, v)| {
            let name = k.strip_prefix(key)?.strip_prefix('.')?;
            Some((name, v.as_str()))
        })
    }

    fn required(&self, key: &str, what: &str) -> Result<&str, String> {
        self.get(key)
            .ok_or_else(|| format!("vars.{key} is not set: {what}"))
    }

    fn port(&self, key: &str, default: Option<u16>) -> Result<u16, String> {
        match self.get(key) {
            None => default.ok_or_else(|| format!("vars.{key} is not set: the port")),
            Some(text) => match text.parse::<u16>() {
                Ok(p) if p > 0 => Ok(p),
                _ => Err(format!("vars.{key} {text:?}: expected a port, 1 to 65535")),
            },
        }
    }

    /// `true`/`false` (also `yes`/`no`, `1`/`0`).
    fn flag(&self, key: &str, default: bool) -> Result<bool, String> {
        match self.get(key).map(str::to_ascii_lowercase).as_deref() {
            None => Ok(default),
            Some("true" | "yes" | "1") => Ok(true),
            Some("false" | "no" | "0") => Ok(false),
            Some(other) => Err(format!("vars.{key} {other:?}: expected true or false")),
        }
    }

    /// A response time: a duration (`500ms`, `2s`) or plain seconds (`0.5`).
    fn seconds(&self, key: &str) -> Result<Option<Duration>, String> {
        let Some(text) = self.get(key) else {
            return Ok(None);
        };
        let err =
            || format!("vars.{key} {text:?}: expected seconds or a duration, e.g. 2 or 500ms");
        if let Ok(secs) = text.parse::<f64>() {
            return if secs.is_finite() && secs >= 0.0 {
                Ok(Some(Duration::from_secs_f64(secs)))
            } else {
                Err(err())
            };
        }
        humantime::parse_duration(text).map(Some).map_err(|_| err())
    }

    fn number(&self, key: &str, default: i64) -> Result<i64, String> {
        match self.get(key) {
            None => Ok(default),
            Some(text) => text
                .parse()
                .map_err(|_| format!("vars.{key} {text:?}: expected a whole number")),
        }
    }
}

/// WARNING and CRITICAL limits for a response time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TimeLimits {
    pub warn: Option<Duration>,
    pub crit: Option<Duration>,
}

impl TimeLimits {
    fn from_vars(vars: &Vars<'_>) -> Result<Self, String> {
        Ok(TimeLimits {
            warn: vars.seconds("warn")?,
            crit: vars.seconds("crit")?,
        })
    }

    /// 0, 1 or 2 for a response that took `took`.
    fn code(&self, took: Duration) -> i32 {
        if self.crit.is_some_and(|c| took >= c) {
            2
        } else if self.warn.is_some_and(|w| took >= w) {
            1
        } else {
            0
        }
    }

    /// `time=0.012s;1;5;0`.
    fn perfdata(&self, took: Duration) -> String {
        let limit = |d: Option<Duration>| d.map(secs).unwrap_or_default();
        format!(
            "time={}s;{};{};0",
            secs(took),
            limit(self.warn),
            limit(self.crit)
        )
    }

    /// ` -w 1 -c 5`, for the command shown in the UI.
    fn args(&self) -> String {
        let mut out = String::new();
        if let Some(w) = self.warn {
            out += &format!(" warn={}", secs(w));
        }
        if let Some(c) = self.crit {
            out += &format!(" crit={}", secs(c));
        }
        out
    }
}

/// Seconds with up to six decimals: `0.012061`, `5`.
fn secs(d: Duration) -> String {
    let text = format!("{:.6}", d.as_secs_f64());
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// `12 ms`, `1.5 s`: a response time for the status line.
fn took(d: Duration) -> String {
    let ms = d.as_secs_f64() * 1000.0;
    if ms < 10.0 {
        format!("{ms:.1} ms")
    } else if ms < 1000.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{:.2} s", ms / 1000.0)
    }
}

fn label(code: i32) -> &'static str {
    match code {
        0 => "OK",
        1 => "WARNING",
        2 => "CRITICAL",
        _ => "UNKNOWN",
    }
}

/// The worse of two exit codes (UNKNOWN counts below CRITICAL).
fn worst(a: i32, b: i32) -> i32 {
    let rank = |c: i32| match c {
        0 => 0,
        1 => 1,
        3 => 2,
        _ => 3,
    };
    if rank(a) >= rank(b) { a } else { b }
}

/// An address for URLs and messages: IPv6 in brackets.
fn host_part(address: &str) -> String {
    match address.parse::<IpAddr>() {
        Ok(IpAddr::V6(v6)) => format!("[{v6}]"),
        _ => address.to_string(),
    }
}

/// The message of an error and its sources: `connect: Connection refused`.
fn chain(err: &dyn std::error::Error) -> String {
    let mut out = err.to_string();
    let mut source = err.source();
    while let Some(s) = source {
        let text = s.to_string();
        if !out.contains(&text) {
            out += &format!(": {text}");
        }
        source = s.source();
    }
    out
}

/// The first line of `text`, cut to 120 characters, for status lines.
fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    match line.char_indices().nth(120) {
        Some((i, _)) => format!("{}…", &line[..i]),
        None => line.to_string(),
    }
}

async fn resolve(address: &str) -> Result<IpAddr, String> {
    if let Ok(ip) = address.parse() {
        return Ok(ip);
    }
    let mut addrs = tokio::net::lookup_host((address, 0))
        .await
        .map_err(|e| format!("cannot resolve: {e}"))?;
    addrs
        .next()
        .map(|a| a.ip())
        .ok_or_else(|| "cannot resolve: no address".into())
}

#[cfg(test)]
pub(crate) fn vars(list: &[(&str, &str)]) -> BTreeMap<String, String> {
    list.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn var_parsing() {
        let map = vars(&[
            ("a", " 0.5 "),
            ("b", "250ms"),
            ("c", "soon"),
            ("p", "70000"),
            ("f", "Yes"),
            ("blank", "  "),
        ]);
        let v = Vars(&map);
        assert_eq!(v.seconds("a").unwrap(), Some(Duration::from_millis(500)));
        assert_eq!(v.seconds("b").unwrap(), Some(Duration::from_millis(250)));
        assert!(v.seconds("c").unwrap_err().contains("vars.c"));
        assert_eq!(v.seconds("blank").unwrap(), None);
        assert!(v.port("p", None).is_err());
        assert_eq!(v.port("missing", Some(443)).unwrap(), 443);
        assert!(
            v.port("missing", None)
                .unwrap_err()
                .contains("vars.missing")
        );
        assert!(v.flag("f", false).unwrap());
        assert!(v.flag("c", false).is_err());
    }

    #[test]
    fn time_limits() {
        let l = TimeLimits {
            warn: Some(Duration::from_secs(1)),
            crit: Some(Duration::from_secs(5)),
        };
        assert_eq!(l.code(Duration::from_millis(999)), 0);
        assert_eq!(l.code(Duration::from_secs(1)), 1);
        assert_eq!(l.code(Duration::from_secs(6)), 2);
        assert_eq!(
            l.perfdata(Duration::from_micros(12_061)),
            "time=0.012061s;1;5;0"
        );
        assert_eq!(
            TimeLimits::default().perfdata(Duration::from_millis(3)),
            "time=0.003s;;;0"
        );
        assert_eq!(worst(0, 2), 2);
        assert_eq!(worst(3, 1), 3);
        assert_eq!(worst(2, 3), 2);
        assert_eq!(took(Duration::from_micros(2_345)), "2.3 ms");
        assert_eq!(took(Duration::from_millis(1_500)), "1.50 s");
        assert_eq!(host_part("::1"), "[::1]");
        assert_eq!(
            first_line("SSH-2.0-OpenSSH_9.6\r\nmore"),
            "SSH-2.0-OpenSSH_9.6"
        );
    }

    #[test]
    fn address_var() {
        let b = Builtin::new("tls", "websites", &vars(&[("address", "shop.example.com")])).unwrap();
        assert_eq!(
            b.to_string(),
            "tls shop.example.com:443 warn=30d crit=14d (built-in)"
        );
        let b = Builtin::new("http", "10.0.0.5", &vars(&[("address", "::1")])).unwrap();
        assert_eq!(b.to_string(), "http GET http://[::1]/ (built-in)");
    }

    #[test]
    fn unknown_builtin() {
        assert!(
            Builtin::new("smtp", "h", &vars(&[]))
                .unwrap_err()
                .contains("known: ping, tcp, http, dns, tls")
        );
    }
}
