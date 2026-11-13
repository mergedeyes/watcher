//! `proxmox-*`: Proxmox VE over its API, with an API token from the
//! secrets. Six built-in checks: the cluster's quorum, a node's load, the
//! storages, one guest (VM or container), backups, and pending updates.
//! Where the API is and how to log in comes from the host's `proxmox`
//! settings ([`Settings`]), not from vars.

use super::{Vars, chain, host_part, label, worst};
use crate::ssh::{Credential, Pool};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const DEFAULT_PORT: u16 = 8006;

/// The names of the Proxmox VE checks.
pub const NAMES: [&str; 6] = [
    "proxmox-cluster",
    "proxmox-node",
    "proxmox-storage",
    "proxmox-guest",
    "proxmox-backup",
    "proxmox-updates",
];

/// What the check looks at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Quorum and nodes online.
    Cluster,
    /// CPU, memory, swap, load and uptime of a node.
    Node,
    /// Usage of the storages, in percent.
    Storage {
        storage: Option<String>,
        warn: u8,
        crit: u8,
    },
    /// One VM or container: running or not, and its load.
    Guest { vmid: u32 },
    /// Failed or missing backups, and guests no backup job covers.
    Backup { hours: u32 },
    /// Pending package updates on a node.
    Updates { warn: u32, crit: Option<u32> },
}

impl Mode {
    /// The check's name: `proxmox-node`.
    pub fn name(&self) -> &'static str {
        match self {
            Mode::Cluster => "proxmox-cluster",
            Mode::Node => "proxmox-node",
            Mode::Storage { .. } => "proxmox-storage",
            Mode::Guest { .. } => "proxmox-guest",
            Mode::Backup { .. } => "proxmox-backup",
            Mode::Updates { .. } => "proxmox-updates",
        }
    }

    /// For messages: `proxmox-node (built-in)`.
    pub fn program(&self) -> &'static str {
        match self {
            Mode::Cluster => "proxmox-cluster (built-in)",
            Mode::Node => "proxmox-node (built-in)",
            Mode::Storage { .. } => "proxmox-storage (built-in)",
            Mode::Guest { .. } => "proxmox-guest (built-in)",
            Mode::Backup { .. } => "proxmox-backup (built-in)",
            Mode::Updates { .. } => "proxmox-updates (built-in)",
        }
    }
}

/// How the server's certificate is checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Trust {
    /// Against the system's trust store, and the name.
    System,
    /// Only its SHA-256 fingerprint, as Proxmox VE shows it. Fits the
    /// self-signed certificates a new installation has.
    Fingerprint([u8; 32]),
    /// Any certificate.
    Any,
}

/// The host's `proxmox` settings, as the checks need them: of the host
/// itself, or for a guest, of the Proxmox VE host it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// The name of the API token in the secrets.
    pub secret: String,
    pub port: u16,
    pub fingerprint: Option<String>,
    pub verify: bool,
    /// For guests: their VM ID, the default of `proxmox-guest`.
    pub vmid: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proxmox {
    pub connection: Connection,
    pub mode: Mode,
    /// The node for `node`, `updates` and (to narrow it) `storage` and
    /// `backup`; without it, the only node if there is just one.
    pub node: Option<String>,
}

impl Proxmox {
    /// Check `name` (one of [`NAMES`]) of the API at `address`, with the
    /// host's `settings`.
    pub(super) fn new(
        name: &str,
        address: &str,
        vars: &Vars<'_>,
        settings: Option<&Settings>,
    ) -> Result<Self, String> {
        let Some(s) = settings else {
            return Err(format!(
                "{name} needs proxmox = {{ secret = \"...\" }} on the host or a host group \
                 (for a VM or container: proxmox = {{ host = \"<Proxmox VE host>\", vmid = 100 }})"
            ));
        };
        let mode = match name {
            "proxmox-cluster" => Mode::Cluster,
            "proxmox-node" => Mode::Node,
            "proxmox-storage" => {
                let warn = percent(vars, "warn", 80)?;
                let crit = percent(vars, "crit", 90)?;
                Mode::Storage {
                    storage: vars.get("storage").map(name_part("storage")).transpose()?,
                    warn,
                    crit,
                }
            }
            "proxmox-guest" => {
                let vmid =
                    match vars.get("vmid") {
                        Some(text) => Some(text.parse::<u32>().map_err(|_| {
                            format!("vars.vmid {text:?}: expected an ID from 100 up")
                        })?),
                        None => s.vmid,
                    };
                match vmid {
                    Some(vmid) if vmid >= 100 => Mode::Guest { vmid },
                    Some(vmid) => return Err(format!("vmid {vmid}: expected an ID from 100 up")),
                    None => {
                        return Err(
                            "proxmox-guest needs the VM ID: proxmox = { host = \"...\", vmid = 100 } \
                             on the host, or vars.vmid"
                                .into(),
                        );
                    }
                }
            }
            "proxmox-backup" => match vars.number("hours", 24)? {
                h @ 1..=8784 => Mode::Backup { hours: h as u32 },
                h => return Err(format!("vars.hours {h}: expected 1 to 8784 (a year)")),
            },
            "proxmox-updates" => {
                let count = |key: &str| -> Result<Option<u32>, String> {
                    vars.get(key)
                        .map(|t| {
                            t.parse::<u32>()
                                .map_err(|_| format!("vars.{key} {t:?}: expected a count"))
                        })
                        .transpose()
                };
                Mode::Updates {
                    warn: count("warn")?.unwrap_or(1),
                    crit: count("crit")?,
                }
            }
            other => return Err(format!("unknown Proxmox VE check {other:?}")),
        };
        let connection = Connection::new(
            address,
            s.port,
            &s.secret,
            s.fingerprint.as_deref(),
            s.verify,
        )?;
        Ok(Proxmox {
            connection,
            mode,
            node: vars.get("node").map(name_part("node")).transpose()?,
        })
    }

    pub(super) async fn run(&self, wait: Duration, secrets: Option<&Pool>) -> (i32, String) {
        let unknown = |text: String| (3, format!("PROXMOX UNKNOWN - {text}"));
        let Some(secrets) = secrets else {
            return unknown("secrets aren't available here (needs the server's database)".into());
        };
        let c = &self.connection;
        let api = match Api::open(&c.url, &c.secret, c.trust, secrets, wait).await {
            Ok(api) => api,
            Err(e) => return unknown(e),
        };
        let report = match tokio::time::timeout(wait, self.check(&api)).await {
            Ok(Ok(report)) => report,
            Ok(Err(e)) => return unknown(e),
            Err(_) => {
                return unknown(format!(
                    "{}: no complete answer within {}",
                    c.url,
                    super::took(wait)
                ));
            }
        };
        (report.code, report.output())
    }

    async fn check(&self, api: &Api<'_>) -> Result<Report, String> {
        match &self.mode {
            Mode::Cluster => cluster(api).await,
            Mode::Node => node(api, &self.node(api).await?).await,
            Mode::Storage {
                storage,
                warn,
                crit,
            } => storages(api, self.node.as_deref(), storage.as_deref(), *warn, *crit).await,
            Mode::Guest { vmid } => guest(api, *vmid).await,
            Mode::Backup { hours } => backup(api, self.node.as_deref(), *hours).await,
            Mode::Updates { warn, crit } => {
                updates(api, &self.node(api).await?, *warn, *crit).await
            }
        }
    }

    /// `vars.node`, or the only node there is.
    async fn node(&self, api: &Api<'_>) -> Result<String, String> {
        if let Some(node) = &self.node {
            return Ok(node.clone());
        }
        let nodes = node_names(api).await?;
        match nodes.as_slice() {
            [one] => Ok(one.clone()),
            [] => Err("the API lists no nodes".into()),
            many => Err(format!(
                "{} nodes ({}): set vars.node",
                many.len(),
                many.join(", ")
            )),
        }
    }
}

impl fmt::Display for Proxmox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.mode.name(), self.connection.url)?;
        if let Some(node) = &self.node {
            write!(f, " node={node}")?;
        }
        match &self.mode {
            Mode::Storage {
                storage,
                warn,
                crit,
            } => {
                if let Some(s) = storage {
                    write!(f, " storage={s}")?;
                }
                write!(f, " warn={warn}% crit={crit}%")?;
            }
            Mode::Guest { vmid } => write!(f, " vmid={vmid}")?,
            Mode::Backup { hours } => write!(f, " hours={hours}")?,
            Mode::Updates { warn, crit } => {
                write!(f, " warn={warn}")?;
                if let Some(c) = crit {
                    write!(f, " crit={c}")?;
                }
            }
            Mode::Cluster | Mode::Node => {}
        }
        // The secret's name only; the token never leaves the vault.
        write!(f, " secret={}", self.connection.secret)?;
        match self.connection.trust {
            Trust::System => Ok(()),
            Trust::Fingerprint(fp) => write!(f, " fingerprint={}", hex_colons(&fp[..4]) + ":…"),
            Trust::Any => f.write_str(" verify=false"),
        }
    }
}

/// A result being put together: state, status line, more lines, metrics.
#[derive(Debug, Default)]
struct Report {
    code: i32,
    text: String,
    details: Vec<String>,
    perf: Vec<String>,
}

impl Report {
    fn raise(&mut self, code: i32) {
        self.code = worst(self.code, code);
    }

    fn output(&self) -> String {
        let mut out = format!("PROXMOX {} - {}", label(self.code), self.text);
        if !self.perf.is_empty() {
            out += "|";
            out += &self.perf.join(" ");
        }
        for line in &self.details {
            out += "\n";
            out += line;
        }
        out
    }
}

struct Api<'a> {
    client: reqwest::Client,
    url: &'a str,
    /// The Authorization header.
    token: String,
    wait: Duration,
}

impl<'a> Api<'a> {
    /// The API at `url`, logged in with the token stored as `secret`.
    async fn open(
        url: &'a str,
        secret: &str,
        trust: Trust,
        secrets: &Pool,
        wait: Duration,
    ) -> Result<Api<'a>, String> {
        let token = match secrets.credential(secret).await? {
            Credential::Token { id, secret } if !id.is_empty() => {
                format!("PVEAPIToken={id}={secret}")
            }
            Credential::Token { .. } => {
                return Err(format!(
                    "secret {secret:?} has no token ID (user@realm!name)"
                ));
            }
            // Easily mixed up when storing it, so say how to fix it.
            other => {
                return Err(format!(
                    "secret {secret:?} is a {}, not an API token: store it as one with \
                     Replace (Configuration → Secrets → API Token, or watcher secret token \
                     {secret} --id 'user@realm!name' --replace)",
                    other.kind(),
                ));
            }
        };
        let client = client(trust).map_err(|e| format!("no HTTP client: {e}"))?;
        Ok(Api {
            client,
            url,
            token,
            wait,
        })
    }

    /// `data` of `GET /api2/json<path>`.
    async fn get(&self, path: &str) -> Result<Value, String> {
        let url = format!("{}/api2/json{path}", self.url);
        let response = self
            .client
            .get(&url)
            .header("Authorization", &self.token)
            .timeout(self.wait)
            .send()
            .await
            .map_err(|e| format!("{}: {}", self.url, error_text(&e)))?;
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            // Proxmox puts the reason into `message` (permission checks) or
            // the status line (wrong token).
            let message = body
                .get("message")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .map(String::from)
                .unwrap_or_else(|| status.canonical_reason().unwrap_or_default().to_string());
            return Err(match status.as_u16() {
                401 => format!("the API refused the token (401 {message})"),
                403 => format!("the token lacks a permission: {message}"),
                code => format!("GET {path}: {code} {message}"),
            });
        }
        match body.get("data") {
            Some(data) => Ok(data.clone()),
            None => Err(format!("GET {path}: no data in the answer")),
        }
    }

    async fn list(&self, path: &str) -> Result<Vec<Value>, String> {
        match self.get(path).await? {
            Value::Array(items) => Ok(items),
            Value::Null => Ok(Vec::new()),
            _ => Err(format!("GET {path}: expected a list")),
        }
    }
}

async fn node_names(api: &Api<'_>) -> Result<Vec<String>, String> {
    let mut names: Vec<String> = api
        .list("/nodes")
        .await?
        .iter()
        .filter_map(|n| text(n, "node"))
        .collect();
    names.sort();
    Ok(names)
}

async fn cluster(api: &Api<'_>) -> Result<Report, String> {
    let entries = api.list("/cluster/status").await?;
    let mut report = Report::default();
    let nodes: Vec<&Value> = entries
        .iter()
        .filter(|e| text(e, "type").as_deref() == Some("node"))
        .collect();
    let offline: Vec<String> = nodes
        .iter()
        .filter(|n| number(n, "online") != Some(1.0))
        .filter_map(|n| text(n, "name"))
        .collect();
    let online = nodes.len() - offline.len();
    let cluster = entries
        .iter()
        .find(|e| text(e, "type").as_deref() == Some("cluster"));
    match cluster {
        // A single node without a cluster has no quorum to lose.
        None => {
            let name = nodes
                .first()
                .and_then(|n| text(n, "name"))
                .unwrap_or_default();
            report.text = format!("{name}: standalone node, no cluster");
        }
        Some(c) => {
            let name = text(c, "name").unwrap_or_default();
            let quorate = number(c, "quorate") == Some(1.0);
            report.text = format!(
                "cluster {name} {}, {online} of {} nodes online",
                if quorate { "quorate" } else { "NOT quorate" },
                nodes.len()
            );
            if !quorate {
                report.raise(2);
            }
        }
    }
    if !offline.is_empty() {
        report.raise(1);
        report.text += &format!(" (offline: {})", offline.join(", "));
    }
    report.perf.push(format!(
        "nodes_online={online};;;0;{total} nodes_offline={off};;;0;{total}",
        total = nodes.len(),
        off = offline.len()
    ));
    Ok(report)
}

async fn node(api: &Api<'_>, name: &str) -> Result<Report, String> {
    // The node list says whether it is online; its status page doesn't
    // answer at all when it isn't.
    let nodes = api.list("/nodes").await?;
    let Some(entry) = nodes
        .iter()
        .find(|n| text(n, "node").as_deref() == Some(name))
    else {
        return Err(format!("no node {name:?} (nodes: {})", names_of(&nodes)));
    };
    let mut report = Report::default();
    if text(entry, "status").as_deref() != Some("online") {
        report.raise(2);
        report.text = format!(
            "{name} is {}",
            text(entry, "status").unwrap_or_else(|| "not online".into())
        );
        return Ok(report);
    }
    let status = api.get(&format!("/nodes/{name}/status")).await?;
    let cpu = number(&status, "cpu").unwrap_or_default() * 100.0;
    let mem_total = number_at(&status, &["memory", "total"]).unwrap_or_default();
    let mem_used = number_at(&status, &["memory", "used"]).unwrap_or_default();
    let swap_total = number_at(&status, &["swap", "total"]).unwrap_or_default();
    let swap_used = number_at(&status, &["swap", "used"]).unwrap_or_default();
    let load: Vec<f64> = status
        .get("loadavg")
        .and_then(Value::as_array)
        .map(|l| l.iter().filter_map(as_number).collect())
        .unwrap_or_default();
    let uptime = number(&status, "uptime").unwrap_or_default();

    let mut text = format!(
        "{name}: CPU {}%, memory {}% ({} of {})",
        dec(cpu, 1),
        dec(pct(mem_used, mem_total), 1),
        bytes(mem_used),
        bytes(mem_total)
    );
    if swap_total > 0.0 {
        text += &format!(", swap {}%", dec(pct(swap_used, swap_total), 1));
    }
    if let Some(l) = load.first() {
        text += &format!(", load {}", dec(*l, 2));
    }
    text += &format!(", up {}", days(uptime));
    report.text = text;

    let perf = &mut report.perf;
    perf.push(format!("cpu={}%;;;0;100", dec(cpu, 2)));
    perf.push(format!(
        "memory={}%;;;0;100",
        dec(pct(mem_used, mem_total), 2)
    ));
    perf.push(format!("memory_used={mem_used:.0}B;;;0;{mem_total:.0}"));
    if swap_total > 0.0 {
        perf.push(format!(
            "swap={}%;;;0;100",
            dec(pct(swap_used, swap_total), 2)
        ));
    }
    for (label, value) in ["load1", "load5", "load15"].iter().zip(&load) {
        perf.push(format!("{label}={}", dec(*value, 2)));
    }
    if let Some(wait) = number(&status, "wait") {
        perf.push(format!("iowait={}%;;;0;100", dec(wait * 100.0, 2)));
    }
    if let (Some(used), Some(total)) = (
        number_at(&status, &["rootfs", "used"]),
        number_at(&status, &["rootfs", "total"]),
    ) {
        perf.push(format!("rootfs={}%;;;0;100", dec(pct(used, total), 2)));
    }
    perf.push(format!("uptime={uptime:.0}s"));
    Ok(report)
}

async fn storages(
    api: &Api<'_>,
    node: Option<&str>,
    only: Option<&str>,
    warn: u8,
    crit: u8,
) -> Result<Report, String> {
    let list = api.list("/cluster/resources?type=storage").await?;
    let mut report = Report::default();
    let mut seen_shared = Vec::new();
    let mut problems = Vec::new();
    let mut fullest: Option<(String, f64)> = None;
    let mut count = 0;
    for s in &list {
        let (Some(name), Some(on)) = (text(s, "storage"), text(s, "node")) else {
            continue;
        };
        if node.is_some_and(|n| n != on) || only.is_some_and(|o| o != name) {
            continue;
        }
        // A shared storage is listed once per node, with the same numbers.
        let shared = number(s, "shared") == Some(1.0);
        if shared {
            if seen_shared.contains(&name) {
                continue;
            }
            seen_shared.push(name.clone());
        }
        count += 1;
        // Names stay the same when nodes come and go: storages of one node
        // are `node/storage`, unless the check is for one node.
        let label = if shared || node.is_some() {
            name.clone()
        } else {
            format!("{on}/{name}")
        };
        if text(s, "status").as_deref() != Some("available") {
            report.raise(2);
            problems.push(format!(
                "{label} {}",
                text(s, "status").unwrap_or_else(|| "not available".into())
            ));
            continue;
        }
        let used = number(s, "disk").unwrap_or_default();
        let total = number(s, "maxdisk").unwrap_or_default();
        if total <= 0.0 {
            continue;
        }
        let p = pct(used, total);
        let code = if p >= f64::from(crit) {
            2
        } else if p >= f64::from(warn) {
            1
        } else {
            0
        };
        if code > 0 {
            report.raise(code);
            problems.push(format!(
                "{label} {}% ({} of {})",
                dec(p, 1),
                bytes(used),
                bytes(total)
            ));
        }
        if fullest.as_ref().is_none_or(|(_, f)| p > *f) {
            fullest = Some((label.clone(), p));
        }
        let quoted = quote(&label);
        report
            .perf
            .push(format!("{quoted}={}%;{warn};{crit};0;100", dec(p, 2)));
        report.perf.push(format!(
            "{}={used:.0}B;;;0;{total:.0}",
            quote(&format!("{label}_used"))
        ));
    }
    if count == 0 {
        return Err(match (only, node) {
            (Some(s), _) => format!("no storage {s:?}{}", on_node(node)),
            (None, _) => format!("no storages{}", on_node(node)),
        });
    }
    report.text = match (problems.is_empty(), fullest) {
        (false, _) => problems.join(", "),
        (true, Some((label, p))) if count == 1 => format!("{label} {}% used", dec(p, 1)),
        (true, Some((label, p))) => {
            format!("{count} storages, fullest {label} {}% used", dec(p, 1))
        }
        (true, None) => format!("{count} storages"),
    };
    Ok(report)
}

fn on_node(node: Option<&str>) -> String {
    node.map(|n| format!(" on node {n}")).unwrap_or_default()
}

async fn guest(api: &Api<'_>, vmid: u32) -> Result<Report, String> {
    let list = api.list("/cluster/resources?type=vm").await?;
    let Some(g) = list
        .iter()
        .find(|g| number(g, "vmid") == Some(f64::from(vmid)))
    else {
        return Err(format!("no VM or container {vmid}"));
    };
    let kind = match text(g, "type").as_deref() {
        Some("lxc") => "CT",
        _ => "VM",
    };
    let name = text(g, "name").unwrap_or_default();
    let on = text(g, "node").unwrap_or_default();
    let status = text(g, "status").unwrap_or_else(|| "unknown".into());
    let mut report = Report::default();
    let who = format!("{kind} {vmid} {name} on {on}");
    if number(g, "template") == Some(1.0) {
        report.text = format!("{who} is a template");
        return Ok(report);
    }
    if status != "running" {
        report.raise(2);
        report.text = format!("{who} is {status}");
        return Ok(report);
    }
    let cores = number(g, "maxcpu").unwrap_or(1.0);
    let cpu = number(g, "cpu").unwrap_or_default() * 100.0;
    let mem = number(g, "mem").unwrap_or_default();
    let maxmem = number(g, "maxmem").unwrap_or_default();
    let uptime = number(g, "uptime").unwrap_or_default();
    let mut text = format!(
        "{who}: running, CPU {}% of {cores:.0} cores, memory {}% ({} of {}), up {}",
        dec(cpu, 1),
        dec(pct(mem, maxmem), 1),
        bytes(mem),
        bytes(maxmem),
        days(uptime)
    );
    if let Some(lock) = text_of(g, "lock") {
        text += &format!(", locked ({lock})");
    }
    report.text = text;
    let perf = &mut report.perf;
    perf.push(format!("cpu={}%;;;0;100", dec(cpu, 2)));
    perf.push(format!("memory={}%;;;0;100", dec(pct(mem, maxmem), 2)));
    perf.push(format!("memory_used={mem:.0}B;;;0;{maxmem:.0}"));
    // Containers report their disk usage; VMs only through the guest agent,
    // and as 0 without it.
    let disk = number(g, "disk").unwrap_or_default();
    let maxdisk = number(g, "maxdisk").unwrap_or_default();
    if kind == "CT" && maxdisk > 0.0 {
        perf.push(format!("disk={}%;;;0;100", dec(pct(disk, maxdisk), 2)));
    }
    perf.push(format!("uptime={uptime:.0}s"));
    for key in ["netin", "netout", "diskread", "diskwrite"] {
        if let Some(v) = number(g, key) {
            perf.push(format!("{key}={v:.0}c"));
        }
    }
    Ok(report)
}

async fn backup(api: &Api<'_>, node: Option<&str>, hours: u32) -> Result<Report, String> {
    let nodes = match node {
        Some(n) => vec![n.to_string()],
        None => node_names(api).await?,
    };
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .saturating_sub(u64::from(hours) * 3600);
    let mut report = Report::default();
    let (mut ok, mut warned, mut failed) = (0, 0, 0);
    for n in &nodes {
        let path = format!("/nodes/{n}/tasks?typefilter=vzdump&since={since}&limit=1000");
        for task in api.list(&path).await? {
            let status = text(&task, "status").unwrap_or_default();
            if status == "OK" {
                ok += 1;
                continue;
            }
            let at = number(&task, "starttime").map_or_else(String::new, |t| {
                humantime::format_rfc3339_seconds(UNIX_EPOCH + Duration::from_secs(t as u64))
                    .to_string()
            });
            let what = match text_of(&task, "id") {
                Some(id) => format!("guest {id}"),
                None => "backup job".to_string(),
            };
            if status.starts_with("WARNINGS") {
                warned += 1;
                report.raise(1);
            } else {
                failed += 1;
                report.raise(2);
            }
            report.details.push(format!(
                "{what} on {n} at {at}: {}",
                super::first_line(&status)
            ));
        }
    }
    let window = format!("in the last {hours} h");
    let mut parts = Vec::new();
    if failed > 0 {
        parts.push(format!("{failed} failed"));
    }
    if warned > 0 {
        parts.push(format!("{warned} with warnings"));
    }
    parts.push(format!("{ok} OK"));
    report.text = format!("backups {window}: {}", parts.join(", "));
    if ok + warned + failed == 0 {
        report.raise(1);
        report.text = format!("no backups ran {window}");
    }
    // Guests that no backup job includes at all.
    let uncovered = api.list("/cluster/backup-info/not-backed-up").await?;
    if !uncovered.is_empty() {
        report.raise(1);
        let list: Vec<String> = uncovered
            .iter()
            .map(|g| {
                let id = number(g, "vmid").map_or_else(String::new, |v| format!("{v:.0}"));
                match text_of(g, "name") {
                    Some(name) => format!("{id} {name}"),
                    None => id,
                }
            })
            .collect();
        report.text += &format!(
            "; {} guest(s) in no backup job: {}",
            list.len(),
            list.join(", ")
        );
    }
    report.perf.push(format!(
        "backups_ok={ok};;;0 backups_warning={warned};;;0 backups_failed={failed};;;0 \
         guests_without_backup={};;;0",
        uncovered.len()
    ));
    Ok(report)
}

async fn updates(
    api: &Api<'_>,
    node: &str,
    warn: u32,
    crit: Option<u32>,
) -> Result<Report, String> {
    let list = api.list(&format!("/nodes/{node}/apt/update")).await?;
    let count = u32::try_from(list.len()).unwrap_or(u32::MAX);
    let mut report = Report::default();
    if crit.is_some_and(|c| count >= c) {
        report.raise(2);
    } else if warn > 0 && count >= warn {
        report.raise(1);
    }
    report.text = match count {
        0 => format!("{node}: no pending updates"),
        1 => format!("{node}: 1 pending update"),
        n => format!("{node}: {n} pending updates"),
    };
    for p in &list {
        if let Some(name) = text(p, "Package") {
            let old = text(p, "OldVersion").unwrap_or_default();
            let new = text(p, "Version").unwrap_or_default();
            report.details.push(format!("{name} {old} → {new}"));
        }
    }
    report.perf.push(format!(
        "updates={count};{warn};{};0",
        crit.map(|c| c.to_string()).unwrap_or_default()
    ));
    Ok(report)
}

/// A Proxmox VE API and the token to log in with, for discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    /// `https://pve.example.com:8006`
    pub url: String,
    /// The name of the token in the secrets.
    pub secret: String,
    pub trust: Trust,
}

/// A VM or container, as discovery finds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guest {
    pub vmid: u32,
    pub name: String,
    /// `qemu` (VM) or `lxc` (container).
    pub kind: String,
    pub node: String,
    pub running: bool,
    /// From the QEMU guest agent or the container's interfaces, while it
    /// runs: the first IPv4 address, else the first IPv6 address that
    /// isn't loopback or link-local.
    pub address: Option<String>,
}

impl Connection {
    /// `fingerprint` as Proxmox VE shows it; without it, `verify` decides
    /// whether the system must trust the certificate.
    pub fn new(
        address: &str,
        port: u16,
        secret: &str,
        fingerprint: Option<&str>,
        verify: bool,
    ) -> Result<Self, String> {
        let trust = match fingerprint.map(parse_fingerprint).transpose()? {
            Some(fp) => Trust::Fingerprint(fp),
            None if verify => Trust::System,
            None => Trust::Any,
        };
        Ok(Connection {
            url: format!("https://{}:{port}", host_part(address)),
            secret: secret.to_string(),
            trust,
        })
    }

    /// The VMs and containers of the cluster, without templates. Each
    /// request may take up to `wait`.
    pub async fn guests(&self, secrets: &Pool, wait: Duration) -> Result<Vec<Guest>, String> {
        let api = Api::open(&self.url, &self.secret, self.trust, secrets, wait).await?;
        let mut guests = Vec::new();
        for g in api.list("/cluster/resources?type=vm").await? {
            if number(&g, "template") == Some(1.0) {
                continue;
            }
            let (Some(vmid), Some(kind), Some(node)) =
                (number(&g, "vmid"), text(&g, "type"), text(&g, "node"))
            else {
                continue;
            };
            let vmid = vmid as u32;
            // Node names go into the paths below; anything odd is skipped.
            if name_part("node")(&node).is_err() || !matches!(kind.as_str(), "qemu" | "lxc") {
                continue;
            }
            let running = text(&g, "status").as_deref() == Some("running");
            // Addresses are only known while the guest runs, and for VMs
            // only with the guest agent. Without them the guest is still
            // listed.
            let address = if running {
                let path = match kind.as_str() {
                    "qemu" => format!("/nodes/{node}/qemu/{vmid}/agent/network-get-interfaces"),
                    _ => format!("/nodes/{node}/lxc/{vmid}/interfaces"),
                };
                api.get(&path).await.ok().and_then(|v| guest_address(&v))
            } else {
                None
            };
            guests.push(Guest {
                vmid,
                name: text_of(&g, "name").unwrap_or_else(|| format!("{kind}-{vmid}")),
                kind,
                node,
                running,
                address,
            });
        }
        guests.sort_by_key(|g| g.vmid);
        Ok(guests)
    }
}

/// The address to reach a guest at, from the agent's answer (`{"result":
/// [interfaces]}`) or a container's interface list.
fn guest_address(answer: &Value) -> Option<String> {
    let interfaces = match answer {
        Value::Array(list) => list.as_slice(),
        other => other.get("result")?.as_array()?.as_slice(),
    };
    let mut v4 = None;
    let mut v6 = None;
    for interface in interfaces {
        if matches!(text(interface, "name").as_deref(), Some("lo" | "lo0")) {
            continue;
        }
        let Some(list) = interface.get("ip-addresses").and_then(Value::as_array) else {
            continue;
        };
        for a in list {
            let Some(ip) = text(a, "ip-address").and_then(|t| t.parse::<IpAddr>().ok()) else {
                continue;
            };
            match ip {
                IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_link_local() => {
                    v4.get_or_insert(ip.to_string());
                }
                IpAddr::V6(ip) if !ip.is_loopback() && !ip.is_unicast_link_local() => {
                    v6.get_or_insert(ip.to_string());
                }
                _ => {}
            }
        }
    }
    v4.or(v6)
}

/// One client per way of trusting the server, kept for all runs: unlike
/// the `http` check, which measures a fresh connection, these keep their
/// connections to the API open between runs.
fn client(trust: Trust) -> Result<reqwest::Client, String> {
    static CLIENTS: OnceLock<Mutex<HashMap<Trust, reqwest::Client>>> = OnceLock::new();
    let clients = CLIENTS.get_or_init(Default::default);
    let mut clients = clients.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = clients.get(&trust) {
        return Ok(c.clone());
    }
    let provider = super::tls::provider();
    let verifier: Arc<dyn ServerCertVerifier> = match trust {
        Trust::System => super::tls::platform_verifier()?,
        Trust::Fingerprint(fingerprint) => Arc::new(Pinned {
            fingerprint: Some(fingerprint),
            provider: provider.clone(),
        }),
        Trust::Any => Arc::new(Pinned {
            fingerprint: None,
            provider: provider.clone(),
        }),
    };
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    let client = reqwest::Client::builder()
        .user_agent(concat!("Watcher/", env!("CARGO_PKG_VERSION")))
        .tls_backend_preconfigured(config)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .pool_idle_timeout(Duration::from_secs(90))
        .build()
        .map_err(|e| chain(&e))?;
    clients.insert(trust, client.clone());
    Ok(client)
}

/// Accepts a certificate by its SHA-256 fingerprint, or any certificate
/// without one. The handshake signatures are still checked, so the server
/// must hold the certificate's key.
#[derive(Debug)]
struct Pinned {
    fingerprint: Option<[u8; 32]>,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let Some(expected) = self.fingerprint else {
            return Ok(ServerCertVerified::assertion());
        };
        let actual: [u8; 32] = Sha256::digest(end_entity.as_ref()).into();
        if actual == expected {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(format!(
                "certificate fingerprint is {}, not the one in proxmox.fingerprint",
                hex_colons(&actual)
            )))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// `18:B4:0D:...` (as Proxmox VE shows it), also without colons and in
/// lower case.
fn parse_fingerprint(text: &str) -> Result<[u8; 32], String> {
    let hex: String = text.chars().filter(|c| *c != ':').collect();
    let err = || {
        format!(
            "fingerprint {text:?}: expected a SHA-256 fingerprint, \
             e.g. 18:B4:0D:… (64 hex digits)"
        )
    };
    if hex.len() != 64 {
        return Err(err());
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hex.get(i * 2..i * 2 + 2).ok_or_else(err)?, 16)
            .map_err(|_| err())?;
    }
    Ok(out)
}

fn hex_colons(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

fn percent(vars: &Vars<'_>, key: &str, default: u8) -> Result<u8, String> {
    match vars.number(key, i64::from(default))? {
        p @ 0..=100 => Ok(p as u8),
        p => Err(format!("vars.{key} {p}: expected a percentage, 0 to 100")),
    }
}

/// Node and storage names go into URL paths: letters, digits, `-`, `_`
/// and `.` only, as Proxmox VE allows for them.
fn name_part(key: &'static str) -> impl Fn(&str) -> Result<String, String> {
    move |text| {
        if text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            Ok(text.to_string())
        } else {
            Err(format!(
                "vars.{key} {text:?}: letters, digits, -, _ and . only"
            ))
        }
    }
}

/// A number, also when Proxmox VE sends it as text (`loadavg`).
fn as_number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn number(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(as_number)
}

fn number_at(v: &Value, path: &[&str]) -> Option<f64> {
    path.iter().try_fold(v, |v, k| v.get(k)).and_then(as_number)
}

fn text(v: &Value, key: &str) -> Option<String> {
    match v.get(key)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Text that is set and not empty.
fn text_of(v: &Value, key: &str) -> Option<String> {
    text(v, key).filter(|s| !s.trim().is_empty())
}

fn names_of(nodes: &[Value]) -> String {
    let names: Vec<String> = nodes.iter().filter_map(|n| text(n, "node")).collect();
    names.join(", ")
}

fn pct(part: f64, whole: f64) -> f64 {
    if whole > 0.0 {
        part / whole * 100.0
    } else {
        0.0
    }
}

/// `12.5`, `3`: at most `places` decimals, without trailing zeros.
fn dec(value: f64, places: usize) -> String {
    let text = format!("{value:.places$}");
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text
    }
}

/// `7.4 GiB`
fn bytes(n: f64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut value = n;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    match unit {
        0 => format!("{value:.0} B"),
        _ => format!("{} {}", dec(value, 1), UNITS[unit]),
    }
}

/// `3 days`, `5 h`, `12 min`
fn days(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    match s {
        0..=3599 => format!("{} min", s / 60),
        3600..=172_799 => format!("{} h", s / 3600),
        _ => format!("{} days", s / 86_400),
    }
}

/// Perfdata labels with spaces or quotes are quoted, as in Nagios.
fn quote(label: &str) -> String {
    if label.contains([' ', '\'', '=']) {
        format!("'{}'", label.replace('\'', "''"))
    } else {
        label.to_string()
    }
}

/// What went wrong, without reqwest's and rustls' wording around it:
/// `invalid peer certificate: UnknownIssuer`, with a hint where one helps.
fn error_text(err: &reqwest::Error) -> String {
    let mut text = chain(err);
    if let Some(i) = text.find("): ")
        && text.starts_with("error sending request")
    {
        text = text[i + 3..].to_string();
    }
    for noise in ["client error (Connect): ", "unexpected error: "] {
        text = text.replace(noise, "");
    }
    // A new Proxmox VE server has a self-signed certificate.
    if text.contains("UnknownIssuer") {
        text += "; for a self-signed certificate, set proxmox.fingerprint on the host";
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::{Builtin, vars};
    use crate::ssh::{BoxFuture, PublicKey, Vault};
    use rustls::ServerConfig;
    use rustls::pki_types::PrivateKeyDer;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::TlsAcceptor;

    const TOKEN: &str = "PVEAPIToken=watcher@pam!mon=0b9a-uuid";

    struct TestVault;

    impl Vault for TestVault {
        fn credential<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Credential, String>> {
            Box::pin(async move {
                match name {
                    "pve" => Credential::token("watcher@pam!mon", "0b9a-uuid"),
                    "wrong" => Credential::token("watcher@pam!mon", "nope"),
                    "pw" => Ok(Credential::Password("x".into())),
                    _ => Err("no such secret".into()),
                }
            })
        }

        fn check_host_key<'a>(
            &'a self,
            _: &'a str,
            _: u16,
            _: &'a PublicKey,
        ) -> BoxFuture<'a, Result<(), String>> {
            Box::pin(async { Err("not used".into()) })
        }
    }

    /// A Proxmox VE API with a self-signed certificate, answering `routes`
    /// (by path with query, else by path alone). Returns the port and the
    /// certificate's fingerprint.
    async fn server(routes: &[(&str, u16, &str)]) -> (u16, String) {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = rcgen::CertificateParams::new(vec!["pve.test".into()])
            .unwrap()
            .self_signed(&key)
            .unwrap();
        let fingerprint = hex_colons(&Sha256::digest(cert.der().as_ref()));
        let config = ServerConfig::builder_with_provider(super::super::tls::provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![cert.der().clone()],
                PrivateKeyDer::Pkcs8(key.serialize_der().into()),
            )
            .unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let routes: Arc<HashMap<String, (u16, String)>> = Arc::new(
            routes
                .iter()
                .map(|(p, code, body)| (p.to_string(), (*code, body.to_string())))
                .collect(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (s, _) = listener.accept().await.unwrap();
                let (acceptor, routes) = (acceptor.clone(), routes.clone());
                tokio::spawn(async move {
                    let Ok(mut tls) = acceptor.accept(s).await else {
                        return;
                    };
                    // Requests one after another on the same connection.
                    loop {
                        let mut head = Vec::new();
                        let mut byte = [0u8; 1];
                        while !head.ends_with(b"\r\n\r\n") {
                            if tls.read(&mut byte).await.unwrap_or(0) == 0 {
                                return;
                            }
                            head.push(byte[0]);
                        }
                        let head = String::from_utf8_lossy(&head).to_string();
                        let target = head.split(' ').nth(1).unwrap_or_default();
                        let path = target.strip_prefix("/api2/json").unwrap_or(target);
                        let authorized = head
                            .lines()
                            .any(|l| l.eq_ignore_ascii_case(&format!("authorization: {TOKEN}")));
                        let (code, body) = if !authorized {
                            (401, "{\"data\":null}".to_string())
                        } else {
                            routes
                                .get(path)
                                .or_else(|| routes.get(path.split('?').next().unwrap_or_default()))
                                .cloned()
                                .unwrap_or((404, "{\"data\":null}".into()))
                        };
                        let response = format!(
                            "HTTP/1.1 {code} X\r\ncontent-type: application/json\r\n\
                             content-length: {}\r\n\r\n{body}",
                            body.len()
                        );
                        if tls.write_all(response.as_bytes()).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        (port, fingerprint)
    }

    /// The host's `proxmox` settings, as the config passes them.
    fn settings(secret: &str, port: u16, fingerprint: Option<&str>) -> Settings {
        Settings {
            secret: secret.into(),
            port,
            fingerprint: fingerprint.map(String::from),
            verify: true,
            vmid: None,
        }
    }

    async fn run(name: &str, list: &[(&str, &str)], s: &Settings) -> (i32, String) {
        let pool = Pool::new(Arc::new(TestVault));
        Builtin::with_proxmox(name, "127.0.0.1", &vars(list), Some(s))
            .unwrap()
            .run(Duration::from_secs(5), Some(&pool))
            .await
    }

    async fn check(
        routes: &[(&str, u16, &str)],
        secret: &str,
        name: &str,
        list: &[(&str, &str)],
    ) -> (i32, String) {
        let (port, fingerprint) = server(routes).await;
        run(name, list, &settings(secret, port, Some(&fingerprint))).await
    }

    // Answers as a Proxmox VE 9.2 server gave them, shortened.
    const NODES: &str = r#"{"data":[{"node":"pve","status":"online","cpu":0.007,"maxcpu":28,
        "mem":7963607040,"maxmem":33532686336,"uptime":8691,"type":"node","id":"node/pve"}]}"#;
    const NODE_STATUS: &str = r#"{"data":{"cpu":0.052,"wait":0.001,
        "loadavg":["0.05","0.13","0.26"],"uptime":8695,
        "memory":{"total":33532686336,"used":7957045248,"free":19316301824},
        "swap":{"free":0,"used":0,"total":0},
        "rootfs":{"total":458483957760,"used":2948464640,"avail":455535493120}}}"#;
    const STORAGE: &str = r#"{"data":[
        {"storage":"hdd","node":"pve","status":"available","disk":28074717184,"maxdisk":983350071296,"shared":0},
        {"storage":"local-zfs","node":"pve","status":"available","disk":390000000000,"maxdisk":459056521216,"shared":0},
        {"storage":"nfs","node":"pve","status":"available","disk":10,"maxdisk":100,"shared":1},
        {"storage":"nfs","node":"pve2","status":"available","disk":10,"maxdisk":100,"shared":1}]}"#;
    const GUESTS: &str = r#"{"data":[
        {"vmid":100,"name":"jellyfin","type":"lxc","node":"pve","status":"running","template":0,
         "cpu":0.25,"maxcpu":4,"mem":311762944,"maxmem":4294967296,"disk":751960064,
         "maxdisk":17179869184,"uptime":7789,"netin":26501672963,"netout":524871732,
         "diskread":102916096,"diskwrite":25863659520},
        {"vmid":101,"name":"opnsense","type":"qemu","node":"pve","status":"stopped","template":0}]}"#;

    #[test]
    fn options() {
        let s = settings("pve", DEFAULT_PORT, None);
        let err = |name: &str, list: &[(&str, &str)]| {
            Builtin::with_proxmox(name, "h", &vars(list), Some(&s)).unwrap_err()
        };
        assert!(
            Builtin::new("proxmox-node", "h", &vars(&[]))
                .unwrap_err()
                .contains("proxmox-node needs proxmox = { secret = \"...\" } on the host")
        );
        assert!(err("proxmox-guest", &[]).contains("proxmox-guest needs the VM ID"));
        assert!(err("proxmox-guest", &[("vmid", "7")]).contains("from 100"));
        assert!(err("proxmox-storage", &[("warn", "120")]).contains("0 to 100"));
        assert!(err("proxmox-node", &[("node", "pve/../x")]).contains("letters, digits"));
        let bad = Settings {
            fingerprint: Some("18:B4".into()),
            ..s.clone()
        };
        assert!(
            Builtin::with_proxmox("proxmox-node", "h", &vars(&[]), Some(&bad))
                .unwrap_err()
                .contains("fingerprint \"18:B4\": expected a SHA-256 fingerprint")
        );

        let any = Settings {
            verify: false,
            ..s.clone()
        };
        let b = Builtin::with_proxmox("proxmox-storage", "::1", &vars(&[]), Some(&any)).unwrap();
        assert_eq!(
            b.to_string(),
            "proxmox-storage https://[::1]:8006 warn=80% crit=90% secret=pve verify=false (built-in)"
        );
        assert_eq!(b.name(), "proxmox-storage");
        // A guest: the VM ID from the host's settings, unless vars.vmid.
        let fp = "18:B4:0D:EC:C4:69:B9:5C:8B:30:C2:82:ED:4A:0C:B9:49:01:0A:3B:04:0F:55:86:68:A8:85:40:6F:22:F3:A2";
        let guest = Settings {
            fingerprint: Some(fp.to_lowercase()),
            vmid: Some(101),
            ..s.clone()
        };
        let b = Builtin::with_proxmox("proxmox-guest", "pve.example.com", &vars(&[]), Some(&guest))
            .unwrap();
        assert_eq!(
            b.to_string(),
            "proxmox-guest https://pve.example.com:8006 vmid=101 secret=pve \
             fingerprint=18:B4:0D:EC:… (built-in)"
        );
        let b = Builtin::with_proxmox(
            "proxmox-guest",
            "pve.example.com",
            &vars(&[("vmid", "102")]),
            Some(&guest),
        )
        .unwrap();
        assert!(b.to_string().contains(" vmid=102 "));
    }

    #[tokio::test]
    async fn node() {
        let routes = [
            ("/nodes", 200, NODES),
            ("/nodes/pve/status", 200, NODE_STATUS),
        ];
        let (code, out) = check(&routes, "pve", "proxmox-node", &[]).await;
        assert_eq!(code, 0, "{out}");
        assert_eq!(
            out,
            "PROXMOX OK - pve: CPU 5.2%, memory 23.7% (7.4 GiB of 31.2 GiB), load 0.05, up 2 h\
             |cpu=5.2%;;;0;100 memory=23.73%;;;0;100 memory_used=7957045248B;;;0;33532686336 \
             load1=0.05 load5=0.13 load15=0.26 iowait=0.1%;;;0;100 rootfs=0.64%;;;0;100 uptime=8695s"
        );
        let offline = NODES.replace("online", "offline");
        let (code, out) = check(&[("/nodes", 200, &offline)], "pve", "proxmox-node", &[]).await;
        assert_eq!(
            (code, out.as_str()),
            (2, "PROXMOX CRITICAL - pve is offline")
        );
        let (code, out) = check(&routes, "pve", "proxmox-node", &[("node", "pve9")]).await;
        assert_eq!(code, 3, "{out}");
        assert!(out.contains("no node \"pve9\" (nodes: pve)"), "{out}");
    }

    #[tokio::test]
    async fn cluster() {
        let standalone = r#"{"data":[{"type":"node","name":"pve","online":1,"local":1}]}"#;
        let (code, out) = check(
            &[("/cluster/status", 200, standalone)],
            "pve",
            "proxmox-cluster",
            &[],
        )
        .await;
        assert_eq!(code, 0, "{out}");
        assert!(
            out.starts_with("PROXMOX OK - pve: standalone node, no cluster|nodes_online=1;"),
            "{out}"
        );
        let lost = r#"{"data":[{"type":"cluster","name":"dc1","quorate":0,"nodes":3},
            {"type":"node","name":"a","online":1},{"type":"node","name":"b","online":0},
            {"type":"node","name":"c","online":0}]}"#;
        let (code, out) = check(
            &[("/cluster/status", 200, lost)],
            "pve",
            "proxmox-cluster",
            &[],
        )
        .await;
        assert_eq!(code, 2, "{out}");
        assert!(
            out.starts_with(
                "PROXMOX CRITICAL - cluster dc1 NOT quorate, 1 of 3 nodes online (offline: b, c)|"
            ),
            "{out}"
        );
    }

    #[tokio::test]
    async fn storage() {
        let routes = [("/cluster/resources?type=storage", 200, STORAGE)];
        let (code, out) = check(&routes, "pve", "proxmox-storage", &[]).await;
        assert_eq!(code, 1, "{out}");
        assert!(
            out.starts_with("PROXMOX WARNING - pve/local-zfs 85% (363.2 GiB of 427.5 GiB)|pve/hdd=2.86%;80;90;0;100 "),
            "{out}"
        );
        // A shared storage once, under its own name.
        assert_eq!(out.matches(" nfs=10%;80;90;0;100").count(), 1, "{out}");
        let (code, out) = check(
            &routes,
            "pve",
            "proxmox-storage",
            &[("storage", "hdd"), ("node", "pve")],
        )
        .await;
        assert_eq!(code, 0, "{out}");
        assert!(
            out.starts_with("PROXMOX OK - hdd 2.9% used|hdd=2.86%;80;90;0;100 hdd_used="),
            "{out}"
        );
        let (code, out) = check(&routes, "pve", "proxmox-storage", &[("storage", "ceph")]).await;
        assert_eq!(
            (code, out.as_str()),
            (3, "PROXMOX UNKNOWN - no storage \"ceph\"")
        );
    }

    #[tokio::test]
    async fn guest() {
        let routes = [("/cluster/resources?type=vm", 200, GUESTS)];
        let (code, out) = check(&routes, "pve", "proxmox-guest", &[("vmid", "100")]).await;
        assert_eq!(code, 0, "{out}");
        assert!(
            out.starts_with(
                "PROXMOX OK - CT 100 jellyfin on pve: running, CPU 25% of 4 cores, memory 7.3% \
                 (297.3 MiB of 4 GiB), up 2 h|cpu=25%;;;0;100 memory=7.26%;;;0;100"
            ),
            "{out}"
        );
        assert!(
            out.contains(" disk=4.38%;;;0;100 uptime=7789s netin=26501672963c"),
            "{out}"
        );
        let (code, out) = check(&routes, "pve", "proxmox-guest", &[("vmid", "101")]).await;
        assert_eq!(
            (code, out.as_str()),
            (2, "PROXMOX CRITICAL - VM 101 opnsense on pve is stopped")
        );
        let (code, out) = check(&routes, "pve", "proxmox-guest", &[("vmid", "102")]).await;
        assert_eq!(
            (code, out.as_str()),
            (3, "PROXMOX UNKNOWN - no VM or container 102")
        );
    }

    #[tokio::test]
    async fn backup() {
        let tasks = r#"{"data":[
            {"type":"vzdump","id":"","node":"pve","starttime":1794610800,"status":"OK"},
            {"type":"vzdump","id":"100","node":"pve","starttime":1794614400,"status":"job errors"}]}"#;
        let uncovered = r#"{"data":[{"vmid":101,"type":"qemu","name":"opnsense"}]}"#;
        let routes = [
            ("/nodes", 200, NODES),
            ("/nodes/pve/tasks", 200, tasks),
            ("/cluster/backup-info/not-backed-up", 200, uncovered),
        ];
        let (code, out) = check(&routes, "pve", "proxmox-backup", &[]).await;
        assert_eq!(code, 2, "{out}");
        assert_eq!(
            out,
            "PROXMOX CRITICAL - backups in the last 24 h: 1 failed, 1 OK; 1 guest(s) in no backup \
             job: 101 opnsense|backups_ok=1;;;0 backups_warning=0;;;0 backups_failed=1;;;0 \
             guests_without_backup=1;;;0\nguest 100 on pve at 2026-11-14T00:00:00Z: job errors"
        );
        let routes = [
            ("/nodes", 200, NODES),
            ("/nodes/pve/tasks", 200, r#"{"data":[]}"#),
            ("/cluster/backup-info/not-backed-up", 200, r#"{"data":[]}"#),
        ];
        let (code, out) = check(&routes, "pve", "proxmox-backup", &[("hours", "168")]).await;
        assert_eq!(code, 1, "{out}");
        assert!(
            out.starts_with("PROXMOX WARNING - no backups ran in the last 168 h|"),
            "{out}"
        );
    }

    #[tokio::test]
    async fn updates() {
        let list =
            r#"{"data":[{"Package":"pve-manager","OldVersion":"9.2.25","Version":"9.2.26"}]}"#;
        let routes = [("/nodes", 200, NODES), ("/nodes/pve/apt/update", 200, list)];
        let (code, out) = check(&routes, "pve", "proxmox-updates", &[]).await;
        assert_eq!(
            (code, out.as_str()),
            (
                1,
                "PROXMOX WARNING - pve: 1 pending update|updates=1;1;;0\npve-manager 9.2.25 → 9.2.26"
            )
        );
        // Listing updates needs Sys.Modify, which PVEAuditor lacks.
        let denied =
            r#"{"message":"Permission check failed (/nodes/pve, Sys.Modify)\n","data":null}"#;
        let routes = [("/nodes/pve/apt/update", 403, denied)];
        let (code, out) = check(&routes, "pve", "proxmox-updates", &[("node", "pve")]).await;
        assert_eq!(
            (code, out.as_str()),
            (
                3,
                "PROXMOX UNKNOWN - the token lacks a permission: Permission check failed (/nodes/pve, Sys.Modify)"
            )
        );
    }

    #[tokio::test]
    async fn refusals() {
        let routes = [("/cluster/status", 200, r#"{"data":[]}"#)];
        let (code, out) = check(&routes, "wrong", "proxmox-cluster", &[]).await;
        assert_eq!(code, 3, "{out}");
        assert!(out.contains("the API refused the token (401"), "{out}");
        let (code, out) = check(&routes, "pw", "proxmox-cluster", &[]).await;
        assert_eq!(
            (code, out.as_str()),
            (
                3,
                "PROXMOX UNKNOWN - secret \"pw\" is a password, not an API token: store it as \
                 one with Replace (Configuration → Secrets → API Token, or watcher secret token pw \
                 --id 'user@realm!name' --replace)"
            )
        );
        let (code, out) = check(&routes, "gone", "proxmox-cluster", &[]).await;
        assert_eq!(
            (code, out.as_str()),
            (3, "PROXMOX UNKNOWN - secret \"gone\": no such secret")
        );

        // Another certificate than the pinned one.
        let (port, _) = server(&routes).await;
        let other = ["00"; 32].join(":");
        let (code, out) = run("proxmox-cluster", &[], &settings("pve", port, Some(&other))).await;
        assert_eq!(code, 3, "{out}");
        assert!(
            out.starts_with(&format!(
                "PROXMOX UNKNOWN - https://127.0.0.1:{port}: certificate fingerprint is "
            )),
            "{out}"
        );
        // Self-signed, so not trusted by the system.
        let (code, out) = run("proxmox-cluster", &[], &settings("pve", port, None)).await;
        assert_eq!(code, 3, "{out}");
        assert!(
            out.ends_with(
                "invalid peer certificate: UnknownIssuer; for a self-signed certificate, \
                 set proxmox.fingerprint on the host"
            ),
            "{out}"
        );
        // Without secrets, as in `run-check`.
        let (code, out) = Builtin::with_proxmox(
            "proxmox-cluster",
            "127.0.0.1",
            &vars(&[]),
            Some(&settings("pve", port, None)),
        )
        .unwrap()
        .run(Duration::from_secs(5), None)
        .await;
        assert_eq!(code, 3, "{out}");
        assert!(out.contains("secrets aren't available here"), "{out}");
    }

    #[tokio::test]
    async fn discovery() {
        // As PVE 9.2 answered for an OPNsense VM (agent) and a container.
        let agent = r#"{"data":{"result":[
            {"name":"vtnet0","ip-addresses":[
                {"ip-address-type":"ipv6","ip-address":"fe80::be24:11ff:feca:5b77","prefix":64},
                {"ip-address":"192.168.178.72","ip-address-type":"ipv4","prefix":24}]},
            {"name":"vtnet1","ip-addresses":[{"ip-address":"10.10.10.1","ip-address-type":"ipv4"}]},
            {"name":"lo0","ip-addresses":[{"ip-address":"127.0.0.1","ip-address-type":"ipv4"}]}]}}"#;
        let interfaces = r#"{"data":[
            {"name":"lo","ip-addresses":[{"ip-address":"127.0.0.1","ip-address-type":"inet"}]},
            {"name":"eth0","ip-addresses":[
                {"ip-address":"2003:d2:6f2c:d300:be24:11ff:fe67:41dd","ip-address-type":"inet6"},
                {"ip-address":"192.168.178.71","ip-address-type":"inet"}]}]}"#;
        let guests = r#"{"data":[
            {"vmid":101,"name":"opnsense","type":"qemu","node":"pve","status":"running","template":0},
            {"vmid":100,"name":"jellyfin","type":"lxc","node":"pve","status":"running","template":0},
            {"vmid":102,"name":"db","type":"qemu","node":"pve","status":"stopped","template":0},
            {"vmid":103,"type":"qemu","node":"pve","status":"running","template":0},
            {"vmid":9000,"name":"debian-tpl","type":"qemu","node":"pve","status":"stopped","template":1}]}"#;
        let (port, fingerprint) = server(&[
            ("/cluster/resources?type=vm", 200, guests),
            (
                "/nodes/pve/qemu/101/agent/network-get-interfaces",
                200,
                agent,
            ),
            ("/nodes/pve/lxc/100/interfaces", 200, interfaces),
            // No guest agent.
            (
                "/nodes/pve/qemu/103/agent/network-get-interfaces",
                500,
                r#"{"message":"QEMU guest agent is not running\n","data":null}"#,
            ),
        ])
        .await;
        let pool = Pool::new(Arc::new(TestVault));
        let connection =
            Connection::new("127.0.0.1", port, "pve", Some(&fingerprint), true).unwrap();
        let found = connection
            .guests(&pool, Duration::from_secs(5))
            .await
            .unwrap();
        let summary: Vec<(u32, &str, &str, bool, Option<&str>)> = found
            .iter()
            .map(|g| {
                (
                    g.vmid,
                    g.name.as_str(),
                    g.kind.as_str(),
                    g.running,
                    g.address.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (100, "jellyfin", "lxc", true, Some("192.168.178.71")),
                (101, "opnsense", "qemu", true, Some("192.168.178.72")),
                (102, "db", "qemu", false, None),
                (103, "qemu-103", "qemu", true, None),
            ]
        );
        let wrong = Connection::new("127.0.0.1", port, "pw", Some(&fingerprint), true).unwrap();
        let err = wrong
            .guests(&pool, Duration::from_secs(5))
            .await
            .unwrap_err();
        assert!(err.contains("is a password, not an API token"), "{err}");
        assert!(Connection::new("h", 8006, "pve", Some("12:34"), true).is_err());
    }

    #[test]
    fn formatting() {
        assert_eq!(dec(12.0, 1), "12");
        assert_eq!(dec(0.126, 2), "0.13");
        assert_eq!(bytes(1023.0), "1023 B");
        assert_eq!(bytes(1536.0 * 1024.0 * 1024.0), "1.5 GiB");
        assert_eq!(days(59.0), "0 min");
        assert_eq!(days(200_000.0), "2 days");
        assert_eq!(quote("pve/my disk"), "'pve/my disk'");
        assert_eq!(parse_fingerprint(&"ab".repeat(32)).unwrap(), [0xab; 32]);
    }
}
