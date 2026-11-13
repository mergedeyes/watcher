//! `dns`: look up a name at a DNS server and check the answer, like
//! `check_dns -H <name> -s <server> -a <expected>`.

use super::{TimeLimits, Vars, chain, host_part, label, resolve, took, worst};
use hickory_resolver::config::{
    ConnectionConfig, NameServerConfig, ResolveHosts, ResolverConfig, ResolverOpts,
};
use hickory_resolver::net::NetError;
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use hickory_resolver::proto::rr::RecordType;
use hickory_resolver::{Resolver, TokioResolver};
use std::fmt;
use std::str::FromStr;
use std::time::{Duration, Instant};

/// Record types that can be asked for.
const TYPES: [&str; 8] = ["A", "AAAA", "CNAME", "MX", "NS", "PTR", "SOA", "TXT"];
/// Answers shown in the status line.
const SHOWN: usize = 5;

/// Where to ask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Server {
    /// The resolvers in /etc/resolv.conf.
    System,
    /// A server by address or name, and port.
    At(String, u16),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dns {
    pub name: String,
    pub record: String,
    pub server: Server,
    /// Values that must all be in the answer.
    pub expect: Vec<String>,
    pub limits: TimeLimits,
}

impl Dns {
    /// Vars: `name` (required), `type` (A), `server` (the host's address,
    /// or `system`), `port` (53), `expect`, `warn`, `crit`.
    pub(super) fn new(address: &str, vars: &Vars<'_>) -> Result<Self, String> {
        let name = vars.required("name", "the name to look up")?.to_string();
        let record = vars.get("type").unwrap_or("A").to_ascii_uppercase();
        if !TYPES.contains(&record.as_str()) {
            return Err(format!(
                "vars.type {record:?}: expected one of {}",
                TYPES.join(", ")
            ));
        }
        let port = vars.port("port", Some(53))?;
        let server = match vars.get("server") {
            Some(s) if s.eq_ignore_ascii_case("system") => Server::System,
            Some(s) => Server::At(s.to_string(), port),
            None => Server::At(address.to_string(), port),
        };
        let expect = vars
            .get("expect")
            .map(|e| {
                e.split(',')
                    .map(normalize)
                    .filter(|v| !v.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        Ok(Dns {
            name,
            record,
            server,
            expect,
            limits: TimeLimits::from_vars(vars)?,
        })
    }

    async fn resolver(&self, wait: Duration) -> Result<TokioResolver, String> {
        let mut builder = match &self.server {
            Server::System => TokioResolver::builder_tokio()
                .map_err(|e| format!("cannot read the system's DNS settings: {e}"))?,
            Server::At(server, port) => {
                let ip = resolve(server)
                    .await
                    .map_err(|e| format!("server {server}: {e}"))?;
                let connections = [ConnectionConfig::udp(), ConnectionConfig::tcp()]
                    .into_iter()
                    .map(|mut c| {
                        c.port = *port;
                        c
                    })
                    .collect();
                let mut ns = NameServerConfig::udp_and_tcp(ip);
                ns.connections = connections;
                Resolver::builder_with_config(
                    ResolverConfig::from_name_servers(vec![ns]),
                    TokioRuntimeProvider::default(),
                )
            }
        };
        let opts: &mut ResolverOpts = builder.options_mut();
        opts.attempts = 2;
        opts.timeout = wait / 2;
        opts.cache_size = 0;
        opts.use_hosts_file = ResolveHosts::Never;
        builder.build().map_err(|e| chain(&e))
    }

    fn server_text(&self) -> String {
        match &self.server {
            Server::System => "system resolver".into(),
            Server::At(s, 53) => s.clone(),
            Server::At(s, port) => format!("{}:{port}", host_part(s)),
        }
    }

    pub(super) async fn run(&self, wait: Duration) -> (i32, String) {
        let at = self.server_text();
        let resolver = match self.resolver(wait).await {
            Ok(r) => r,
            Err(e) => return (3, format!("DNS UNKNOWN - {e}")),
        };
        let record = RecordType::from_str(&self.record).unwrap_or(RecordType::A);
        let what = format!("{} {}", self.name, self.record);
        let start = Instant::now();
        let lookup =
            match tokio::time::timeout(wait, resolver.lookup(self.name.as_str(), record)).await {
                Ok(Ok(l)) => l,
                Ok(Err(e)) => {
                    return (
                        2,
                        format!("DNS CRITICAL - {what}: {} ({at})", lookup_error(&e)),
                    );
                }
                Err(_) => {
                    return (
                        2,
                        format!(
                            "DNS CRITICAL - {what}: no answer within {} ({at})",
                            took(wait)
                        ),
                    );
                }
            };
        let elapsed = start.elapsed();
        let answers: Vec<String> = lookup
            .answers()
            .iter()
            .filter(|r| r.record_type() == record)
            .map(|r| r.data.to_string())
            .collect();
        if answers.is_empty() {
            return (2, format!("DNS CRITICAL - {what}: no records ({at})"));
        }
        let missing: Vec<&String> = self
            .expect
            .iter()
            .filter(|e| !answers.iter().any(|a| matches(a, e)))
            .collect();
        let mut shown = answers
            .iter()
            .take(SHOWN)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        if answers.len() > SHOWN {
            shown += &format!(" and {} more", answers.len() - SHOWN);
        }
        let mut code = self.limits.code(elapsed);
        let mut text = format!("{what} {shown} in {} ({at})", took(elapsed));
        if !missing.is_empty() {
            code = worst(code, 2);
            let list: Vec<&str> = missing.iter().map(|s| s.as_str()).collect();
            text = format!("{what}: {} missing; got {shown} ({at})", list.join(", "));
        }
        (
            code,
            format!(
                "DNS {} - {text}|{}",
                label(code),
                self.limits.perfdata(elapsed)
            ),
        )
    }
}

impl fmt::Display for Dns {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "dns {} {} @{}",
            self.name,
            self.record,
            self.server_text()
        )?;
        if !self.expect.is_empty() {
            write!(f, " expect={}", self.expect.join(","))?;
        }
        f.write_str(&self.limits.args())
    }
}

fn lookup_error(e: &NetError) -> String {
    if e.is_nx_domain() {
        "does not exist (NXDOMAIN)".into()
    } else if e.is_no_records_found() {
        "no records".into()
    } else if matches!(e, NetError::Timeout) {
        "no answer (timeout)".into()
    } else {
        chain(e)
    }
}

/// Lower case, without the final dot: `mail.example.com`.
fn normalize(value: &str) -> String {
    value.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// Whether answer `answer` is the expected value: the whole of it, or its
/// last part (the host of an MX answer `10 mail.example.com.`).
fn matches(answer: &str, expected: &str) -> bool {
    let answer = normalize(answer);
    answer == expected || answer.rsplit(' ').next() == Some(expected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::{Builtin, vars};
    use hickory_resolver::proto::op::{Message, MessageType, ResponseCode};
    use hickory_resolver::proto::rr::rdata::{A, MX};
    use hickory_resolver::proto::rr::{Name, RData, Record};
    use std::net::Ipv4Addr;
    use tokio::net::UdpSocket;

    /// A DNS server on UDP: `www.test.` A 10.0.0.1 and 10.0.0.2, `test.` MX
    /// 10 mail.test., everything else NXDOMAIN.
    async fn server() -> u16 {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = socket.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut buf = [0u8; 512];
            loop {
                let (n, from) = socket.recv_from(&mut buf).await.unwrap();
                let request = Message::from_vec(&buf[..n]).unwrap();
                let mut response = Message::response(request.id, request.op_code);
                response.metadata.recursion_available = true;
                response.metadata.recursion_desired = request.metadata.recursion_desired;
                response.metadata.message_type = MessageType::Response;
                let query = request.queries[0].clone();
                let name = query.name().to_string();
                let answers: Vec<Record> = match (name.as_str(), query.query_type()) {
                    ("www.test.", RecordType::A) => [1, 2]
                        .into_iter()
                        .map(|last| {
                            Record::from_rdata(
                                query.name().clone(),
                                60,
                                RData::A(A(Ipv4Addr::new(10, 0, 0, last))),
                            )
                        })
                        .collect(),
                    ("test.", RecordType::MX) => vec![Record::from_rdata(
                        query.name().clone(),
                        60,
                        RData::MX(MX::new(10, Name::from_ascii("mail.test.").unwrap())),
                    )],
                    _ => {
                        response.metadata.response_code = ResponseCode::NXDomain;
                        Vec::new()
                    }
                };
                response.add_query(query);
                response.add_answers(answers);
                let bytes = response.to_vec().unwrap();
                socket.send_to(&bytes, from).await.unwrap();
            }
        });
        port
    }

    async fn run(list: &[(&str, &str)]) -> (i32, String) {
        Builtin::new("dns", "127.0.0.1", &vars(list))
            .unwrap()
            .run(Duration::from_secs(2), None)
            .await
    }

    #[test]
    fn settings() {
        assert!(
            Builtin::new("dns", "h", &vars(&[]))
                .unwrap_err()
                .contains("vars.name")
        );
        assert!(Builtin::new("dns", "h", &vars(&[("name", "x"), ("type", "SRV")])).is_err());
        let b = Builtin::new(
            "dns",
            "10.0.0.53",
            &vars(&[
                ("name", "example.com"),
                ("type", "mx"),
                ("expect", "Mail.Example.com."),
            ]),
        )
        .unwrap();
        assert_eq!(
            b.to_string(),
            "dns example.com MX @10.0.0.53 expect=mail.example.com (built-in)"
        );
        let b = Builtin::new("dns", "h", &vars(&[("name", "x"), ("server", "system")])).unwrap();
        assert_eq!(b.to_string(), "dns x A @system resolver (built-in)");
        assert!(matches("10 mail.example.com.", "mail.example.com"));
        assert!(!matches("10.0.0.10", "10.0.0.1"));
    }

    #[tokio::test]
    async fn answers_and_expectations() {
        let port = server().await.to_string();
        let (code, out) = run(&[("name", "www.test."), ("port", &port)]).await;
        assert_eq!(code, 0, "{out}");
        assert!(
            out.starts_with("DNS OK - www.test. A 10.0.0.1, 10.0.0.2 in "),
            "{out}"
        );
        assert!(out.contains(&format!("(127.0.0.1:{port})|time=")), "{out}");

        let (code, out) = run(&[
            ("name", "www.test."),
            ("port", &port),
            ("expect", "10.0.0.2"),
        ])
        .await;
        assert_eq!(code, 0, "{out}");
        let (code, out) = run(&[
            ("name", "www.test."),
            ("port", &port),
            ("expect", "10.0.0.2, 10.0.0.3"),
        ])
        .await;
        assert_eq!(code, 2, "{out}");
        assert!(
            out.starts_with("DNS CRITICAL - www.test. A: 10.0.0.3 missing; got 10.0.0.1, 10.0.0.2"),
            "{out}"
        );

        let (code, out) = run(&[
            ("name", "test."),
            ("type", "MX"),
            ("port", &port),
            ("expect", "mail.test"),
        ])
        .await;
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("test. MX 10 mail.test."), "{out}");
    }

    #[tokio::test]
    async fn missing_names_and_silent_servers() {
        let port = server().await.to_string();
        let (code, out) = run(&[("name", "nope.test."), ("port", &port)]).await;
        assert_eq!(code, 2, "{out}");
        assert!(
            out.starts_with("DNS CRITICAL - nope.test. A: does not exist (NXDOMAIN)"),
            "{out}"
        );

        // Nobody answers on this port.
        let silent = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = silent.local_addr().unwrap().port().to_string();
        let (code, out) = Builtin::new(
            "dns",
            "127.0.0.1",
            &vars(&[("name", "www.test."), ("port", &port)]),
        )
        .unwrap()
        .run(Duration::from_millis(800), None)
        .await;
        assert_eq!(code, 2, "{out}");
        drop(silent);
    }
}
