//! Checks that run in-process instead of as a plugin: `ping`.
//!
//! They print plugin output (Nagios format with perfdata) and an exit code,
//! so their results go through the same evaluation as any plugin's.

use std::collections::BTreeMap;
use std::fmt;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;
use surge_ping::{Client, Config, ICMP, PingIdentifier, PingSequence};
use tokio::task::JoinSet;

/// The built-in checks, by name.
pub const NAMES: [&str; 1] = ["ping"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Builtin {
    Ping(Ping),
}

/// ICMP echo, like `check_ping -H <address> -p <packets> -w <warn> -c <crit>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ping {
    pub address: String,
    pub packets: u16,
    pub warn: Limits,
    pub crit: Limits,
}

/// Round-trip average and packet loss, written `3000,80%` (ms, percent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub rta: Duration,
    pub loss: u8,
}

impl Limits {
    fn parse(text: &str) -> Result<Self, String> {
        let err = || format!("{text:?}: expected <rta ms>,<loss>%, e.g. 3000,80%");
        let (rta, loss) = text.split_once(',').ok_or_else(err)?;
        let rta: f64 = rta.trim().parse().map_err(|_| err())?;
        let loss: u8 = loss
            .trim()
            .trim_end_matches('%')
            .parse()
            .map_err(|_| err())?;
        if !rta.is_finite() || rta < 0.0 || loss > 100 {
            return Err(err());
        }
        Ok(Limits {
            rta: Duration::from_secs_f64(rta / 1000.0),
            loss,
        })
    }

    fn reached(&self, rta: Option<Duration>, loss: u8) -> bool {
        loss >= self.loss || rta.is_some_and(|r| r >= self.rta)
    }
}

impl fmt::Display for Limits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{},{}%", ms(self.rta), self.loss)
    }
}

impl Builtin {
    /// Builds built-in check `name` for a host at `address`. Its vars:
    /// `packets`, `warn`, `crit`.
    pub fn new(name: &str, address: &str, vars: &BTreeMap<String, String>) -> Result<Self, String> {
        match name {
            "ping" => {
                let var = |key: &str, default: &str| {
                    vars.get(key).map_or(default, String::as_str).to_string()
                };
                let packets = var("packets", "5");
                let packets = match packets.trim().parse::<u16>() {
                    Ok(n @ 1..=100) => n,
                    _ => return Err(format!("packets {packets:?}: expected 1 to 100")),
                };
                Ok(Builtin::Ping(Ping {
                    address: address.to_string(),
                    packets,
                    warn: Limits::parse(&var("warn", "3000,80%"))
                        .map_err(|e| format!("warn {e}"))?,
                    crit: Limits::parse(&var("crit", "5000,100%"))
                        .map_err(|e| format!("crit {e}"))?,
                }))
            }
            _ => Err(format!(
                "unknown builtin {name:?} (known: {})",
                NAMES.join(", ")
            )),
        }
    }

    /// Runs the check: the exit code and plugin output.
    pub async fn run(&self) -> (i32, String) {
        match self {
            Builtin::Ping(p) => p.run().await,
        }
    }
}

impl fmt::Display for Builtin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Builtin::Ping(p) => write!(
                f,
                "ping -H {} -p {} -w {} -c {} (built-in)",
                p.address, p.packets, p.warn, p.crit
            ),
        }
    }
}

/// Time between echo requests.
const INTERVAL: Duration = Duration::from_millis(200);
const PAYLOAD: [u8; 56] = [0; 56];

/// Identifiers for raw sockets, where replies to every request arrive and
/// are told apart by it. Datagram ICMP sockets (Linux) get theirs from the
/// kernel.
static IDENT: AtomicU16 = AtomicU16::new(0);

impl Ping {
    async fn run(&self) -> (i32, String) {
        let ip = match resolve(&self.address).await {
            Ok(ip) => ip,
            Err(e) => return (2, format!("PING CRITICAL - {}: {e}", self.address)),
        };
        let kind = if ip.is_ipv4() { ICMP::V4 } else { ICMP::V6 };
        // A socket per run: replies to concurrent runs never mix.
        let client = match Client::new(&Config::builder().kind(kind).build()) {
            Ok(c) => c,
            Err(e) => {
                let line = e.to_string().lines().next().unwrap_or_default().to_string();
                return (3, format!("PING UNKNOWN - no ICMP socket: {line}"));
            }
        };
        let ident =
            PingIdentifier(IDENT.fetch_add(1, Ordering::Relaxed) ^ std::process::id() as u16);
        // Slower replies than the critical average count as lost.
        let wait = self
            .crit
            .rta
            .clamp(Duration::from_secs(1), Duration::from_secs(10));
        let mut pings = JoinSet::new();
        for seq in 0..self.packets {
            let client = client.clone();
            pings.spawn(async move {
                tokio::time::sleep(INTERVAL * u32::from(seq)).await;
                let mut pinger = client.pinger(ip, ident).await;
                pinger.timeout(wait);
                pinger
                    .ping(PingSequence(seq), &PAYLOAD)
                    .await
                    .map(|(_, rtt)| rtt)
            });
        }
        let mut rtts = Vec::new();
        let mut error = None;
        for result in pings.join_all().await {
            match result {
                Ok(rtt) => rtts.push(rtt),
                Err(surge_ping::SurgeError::Timeout { .. }) => {}
                Err(e) => error = Some(e.to_string()),
            }
        }

        let sent = u32::from(self.packets);
        let lost = sent - rtts.len() as u32;
        let loss = (lost * 100 / sent) as u8;
        let rta = (!rtts.is_empty()).then(|| rtts.iter().sum::<Duration>() / rtts.len() as u32);
        let (code, label) = if self.crit.reached(rta, loss) {
            (2, "CRITICAL")
        } else if self.warn.reached(rta, loss) {
            (1, "WARNING")
        } else {
            (0, "OK")
        };
        let mut out = format!("PING {label} - Packet loss = {loss}%");
        if let Some(rta) = rta {
            out += &format!(", RTA = {} ms", ms(rta));
        } else if let Some(e) = error {
            out += &format!(" ({e})");
        }
        out += "|";
        if let Some(rta) = rta {
            out += &format!(
                "rta={}ms;{};{};0 ",
                ms(rta),
                ms(self.warn.rta),
                ms(self.crit.rta)
            );
        }
        out += &format!("pl={loss}%;{};{};0", self.warn.loss, self.crit.loss);
        (code, out)
    }
}

/// Milliseconds with up to three decimals: `0.113`, `3000`.
fn ms(d: Duration) -> String {
    let text = format!("{:.3}", d.as_secs_f64() * 1000.0);
    text.trim_end_matches('0').trim_end_matches('.').to_string()
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
mod tests {
    use super::*;

    fn vars(list: &[(&str, &str)]) -> BTreeMap<String, String> {
        list.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn ping_settings_from_vars() {
        let Builtin::Ping(p) = Builtin::new("ping", "h", &vars(&[])).unwrap();
        assert_eq!(
            (p.packets, p.warn.to_string(), p.crit.to_string()),
            (5, "3000,80%".into(), "5000,100%".into())
        );
        let Builtin::Ping(p) = Builtin::new(
            "ping",
            "h",
            &vars(&[("packets", "3"), ("warn", "100.5, 20%")]),
        )
        .unwrap();
        assert_eq!((p.packets, p.warn.rta), (3, Duration::from_micros(100_500)));
        assert!(Builtin::new("ping", "h", &vars(&[("crit", "500")])).is_err());
        assert!(Builtin::new("ping", "h", &vars(&[("packets", "0")])).is_err());
        assert!(Builtin::new("dns", "h", &vars(&[])).is_err());
    }

    #[test]
    fn limits() {
        let l = Limits::parse("100,20%").unwrap();
        assert!(!l.reached(Some(Duration::from_millis(99)), 19));
        assert!(l.reached(Some(Duration::from_millis(100)), 0));
        assert!(l.reached(None, 20));
        assert_eq!(ms(Duration::from_micros(113)), "0.113");
        assert_eq!(ms(Duration::from_secs(3)), "3000");
    }

    #[tokio::test]
    async fn ping_localhost() {
        let Builtin::Ping(p) =
            Builtin::new("ping", "127.0.0.1", &vars(&[("packets", "2")])).unwrap();
        let (code, out) = p.run().await;
        // Without ICMP sockets (ping_group_range, CAP_NET_RAW): UNKNOWN.
        assert!(
            code == 0 || (code == 3 && out.contains("no ICMP socket")),
            "{code} {out}"
        );
        if code == 0 {
            assert!(
                out.starts_with("PING OK - Packet loss = 0%, RTA = "),
                "{out}"
            );
            assert!(
                out.contains("|rta=") && out.ends_with("pl=0%;80;100;0"),
                "{out}"
            );
        }
    }

    #[tokio::test]
    async fn unresolvable_is_critical() {
        let (code, out) = Builtin::new("ping", "nonexistent.invalid", &vars(&[]))
            .unwrap()
            .run()
            .await;
        assert_eq!(code, 2, "{out}");
        assert!(
            out.starts_with("PING CRITICAL - nonexistent.invalid: cannot resolve"),
            "{out}"
        );
    }
}
