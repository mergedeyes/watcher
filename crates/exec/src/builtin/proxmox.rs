//! `proxmox`: Proxmox VE over its API, with an API token from the secrets.
//! One check with several modes: the cluster's quorum, a node's load, the
//! storages, one guest (VM or container), backups, and pending updates.

use super::{Vars, chain, host_part, label, worst};
use crate::ssh::{Credential, Pool};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const DEFAULT_PORT: u16 = 8006;

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
    fn name(&self) -> &'static str {
        match self {
            Mode::Cluster => "cluster",
            Mode::Node => "node",
            Mode::Storage { .. } => "storage",
            Mode::Guest { .. } => "guest",
            Mode::Backup { .. } => "backup",
            Mode::Updates { .. } => "updates",
        }
    }
}

const MODES: &str = "cluster, node, storage, guest, backup, updates";

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proxmox {
    /// `https://pve.example.com:8006`
    pub url: String,
    /// The name of the token in the secrets.
    pub secret: String,
    pub mode: Mode,
    /// The node for `node`, `updates` and (to narrow it) `storage` and
    /// `backup`; without it, the only node if there is just one.
    pub node: Option<String>,
    pub trust: Trust,
}

impl Proxmox {
    /// Vars: `mode`, `secret`, `port` (8006), `node`, `storage`, `vmid`,
    /// `hours`, `warn`, `crit`, `verify`, `fingerprint`.
    pub(super) fn new(address: &str, vars: &Vars<'_>) -> Result<Self, String> {
        let mode_name = vars.required("mode", MODES)?;
        // Vars of other modes are refused, so a typo in `mode` or a var in
        // the wrong place doesn't go unnoticed.
        let takes: &[&str] = match mode_name {
            "cluster" => &[],
            "node" => &["node"],
            "storage" => &["node", "storage", "warn", "crit"],
            "guest" => &["vmid"],
            "backup" => &["node", "hours"],
            "updates" => &["node", "warn", "crit"],
            other => return Err(format!("vars.mode {other:?}: expected one of {MODES}")),
        };
        for key in ["node", "storage", "vmid", "hours", "warn", "crit"] {
            if vars.get(key).is_some() && !takes.contains(&key) {
                return Err(format!(
                    "vars.{key}: not used with mode {mode_name:?} (it takes: {})",
                    if takes.is_empty() {
                        "nothing extra".to_string()
                    } else {
                        takes.join(", ")
                    }
                ));
            }
        }
        let mode = match mode_name {
            "cluster" => Mode::Cluster,
            "node" => Mode::Node,
            "storage" => {
                let warn = percent(vars, "warn", 80)?;
                let crit = percent(vars, "crit", 90)?;
                Mode::Storage {
                    storage: vars.get("storage").map(name_part("storage")).transpose()?,
                    warn,
                    crit,
                }
            }
            "guest" => {
                let text = vars.required("vmid", "the VM or container ID, e.g. 100")?;
                match text.parse::<u32>() {
                    Ok(vmid) if vmid >= 100 => Mode::Guest { vmid },
                    _ => return Err(format!("vars.vmid {text:?}: expected an ID from 100 up")),
                }
            }
            "backup" => match vars.number("hours", 24)? {
                h @ 1..=8784 => Mode::Backup { hours: h as u32 },
                h => return Err(format!("vars.hours {h}: expected 1 to 8784 (a year)")),
            },
            _ => {
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
        };
        let fingerprint = vars.get("fingerprint").map(parse_fingerprint).transpose()?;
        let trust = match (fingerprint, vars.flag("verify", true)?) {
            (Some(fp), _) => Trust::Fingerprint(fp),
            (None, true) => Trust::System,
            (None, false) => Trust::Any,
        };
        let port = vars.port("port", Some(DEFAULT_PORT))?;
        Ok(Proxmox {
            url: format!("https://{}:{port}", host_part(address)),
            secret: vars
                .required(
                    "secret",
                    "the name of the API token, see `watcher secret token`",
                )?
                .to_string(),
            mode,
            node: vars.get("node").map(name_part("node")).transpose()?,
            trust,
        })
    }

    pub(super) async fn run(&self, wait: Duration, secrets: Option<&Pool>) -> (i32, String) {
        let unknown = |text: String| (3, format!("PROXMOX UNKNOWN - {text}"));
        let Some(secrets) = secrets else {
            return unknown("secrets aren't available here (needs the server's database)".into());
        };
        let token = match secrets.credential(&self.secret).await {
            Ok(Credential::Token { id, secret }) if !id.is_empty() => {
                format!("PVEAPIToken={id}={secret}")
            }
            Ok(Credential::Token { .. }) => {
                return unknown(format!(
                    "secret {:?} has no token ID (user@realm!name)",
                    self.secret
                ));
            }
            Ok(other) => {
                // Easily mixed up when storing it, so say how to fix it.
                return unknown(format!(
                    "secret {name:?} is a {}, not an API token: store it as one with \
                     Replace (Configuration → Secrets → API Token, or watcher secret token \
                     {name} --id 'user@realm!name' --replace)",
                    other.kind(),
                    name = self.secret,
                ));
            }
            Err(e) => return unknown(e),
        };
        let client = match client(self.trust) {
            Ok(c) => c,
            Err(e) => return unknown(format!("no HTTP client: {e}")),
        };
        let api = Api {
            client,
            url: &self.url,
            token,
            wait,
        };
        let report = match tokio::time::timeout(wait, self.check(&api)).await {
            Ok(Ok(report)) => report,
            Ok(Err(e)) => return unknown(e),
            Err(_) => {
                return unknown(format!(
                    "{}: no complete answer within {}",
                    self.url,
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
        write!(f, "proxmox {} mode={}", self.url, self.mode.name())?;
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
        write!(f, " secret={}", self.secret)?;
        match self.trust {
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

impl Api<'_> {
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
                "certificate fingerprint is {}, not the one in vars.fingerprint",
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
            "vars.fingerprint {text:?}: expected a SHA-256 fingerprint, \
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
        text += "; for a self-signed certificate, set vars.fingerprint";
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

    async fn check(
        routes: &[(&str, u16, &str)],
        secret: &str,
        list: &[(&str, &str)],
    ) -> (i32, String) {
        let (port, fingerprint) = server(routes).await;
        let port = port.to_string();
        let mut all = vec![
            ("port", port.as_str()),
            ("secret", secret),
            ("fingerprint", fingerprint.as_str()),
        ];
        all.extend_from_slice(list);
        let pool = Pool::new(Arc::new(TestVault));
        Builtin::new("proxmox", "127.0.0.1", &vars(&all))
            .unwrap()
            .run(Duration::from_secs(5), Some(&pool))
            .await
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
    fn settings() {
        let err = |list: &[(&str, &str)]| Builtin::new("proxmox", "h", &vars(list)).unwrap_err();
        assert!(err(&[("secret", "pve")]).contains("vars.mode is not set"));
        assert!(err(&[("mode", "vms"), ("secret", "pve")]).contains("expected one of cluster"));
        assert!(err(&[("mode", "node")]).contains("vars.secret is not set"));
        assert!(
            err(&[("mode", "node"), ("secret", "s"), ("vmid", "100")])
                .contains("vars.vmid: not used with mode \"node\" (it takes: node)")
        );
        assert!(err(&[("mode", "guest"), ("secret", "s")]).contains("vars.vmid is not set"));
        assert!(err(&[("mode", "guest"), ("secret", "s"), ("vmid", "7")]).contains("from 100"));
        assert!(err(&[("mode", "storage"), ("secret", "s"), ("warn", "120")]).contains("0 to 100"));
        assert!(
            err(&[("mode", "node"), ("secret", "s"), ("node", "pve/../x")])
                .contains("letters, digits")
        );
        assert!(
            err(&[("mode", "node"), ("secret", "s"), ("fingerprint", "18:B4")])
                .contains("SHA-256 fingerprint")
        );

        let b = Builtin::new(
            "proxmox",
            "::1",
            &vars(&[("mode", "storage"), ("secret", "pve"), ("verify", "false")]),
        )
        .unwrap();
        assert_eq!(
            b.to_string(),
            "proxmox https://[::1]:8006 mode=storage warn=80% crit=90% secret=pve verify=false (built-in)"
        );
        let fp = "18:B4:0D:EC:C4:69:B9:5C:8B:30:C2:82:ED:4A:0C:B9:49:01:0A:3B:04:0F:55:86:68:A8:85:40:6F:22:F3:A2";
        let b = Builtin::new(
            "proxmox",
            "pve.example.com",
            &vars(&[
                ("mode", "guest"),
                ("vmid", "101"),
                ("secret", "pve"),
                ("fingerprint", &fp.to_lowercase()),
            ]),
        )
        .unwrap();
        assert_eq!(
            b.to_string(),
            "proxmox https://pve.example.com:8006 mode=guest vmid=101 secret=pve \
             fingerprint=18:B4:0D:EC:… (built-in)"
        );
    }

    #[tokio::test]
    async fn node() {
        let routes = [
            ("/nodes", 200, NODES),
            ("/nodes/pve/status", 200, NODE_STATUS),
        ];
        let (code, out) = check(&routes, "pve", &[("mode", "node")]).await;
        assert_eq!(code, 0, "{out}");
        assert_eq!(
            out,
            "PROXMOX OK - pve: CPU 5.2%, memory 23.7% (7.4 GiB of 31.2 GiB), load 0.05, up 2 h\
             |cpu=5.2%;;;0;100 memory=23.73%;;;0;100 memory_used=7957045248B;;;0;33532686336 \
             load1=0.05 load5=0.13 load15=0.26 iowait=0.1%;;;0;100 rootfs=0.64%;;;0;100 uptime=8695s"
        );
        let offline = NODES.replace("online", "offline");
        let (code, out) = check(&[("/nodes", 200, &offline)], "pve", &[("mode", "node")]).await;
        assert_eq!(
            (code, out.as_str()),
            (2, "PROXMOX CRITICAL - pve is offline")
        );
        let (code, out) = check(&routes, "pve", &[("mode", "node"), ("node", "pve9")]).await;
        assert_eq!(code, 3, "{out}");
        assert!(out.contains("no node \"pve9\" (nodes: pve)"), "{out}");
    }

    #[tokio::test]
    async fn cluster() {
        let standalone = r#"{"data":[{"type":"node","name":"pve","online":1,"local":1}]}"#;
        let (code, out) = check(
            &[("/cluster/status", 200, standalone)],
            "pve",
            &[("mode", "cluster")],
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
            &[("mode", "cluster")],
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
        let (code, out) = check(&routes, "pve", &[("mode", "storage")]).await;
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
            &[("mode", "storage"), ("storage", "hdd"), ("node", "pve")],
        )
        .await;
        assert_eq!(code, 0, "{out}");
        assert!(
            out.starts_with("PROXMOX OK - hdd 2.9% used|hdd=2.86%;80;90;0;100 hdd_used="),
            "{out}"
        );
        let (code, out) = check(&routes, "pve", &[("mode", "storage"), ("storage", "ceph")]).await;
        assert_eq!(
            (code, out.as_str()),
            (3, "PROXMOX UNKNOWN - no storage \"ceph\"")
        );
    }

    #[tokio::test]
    async fn guest() {
        let routes = [("/cluster/resources?type=vm", 200, GUESTS)];
        let (code, out) = check(&routes, "pve", &[("mode", "guest"), ("vmid", "100")]).await;
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
        let (code, out) = check(&routes, "pve", &[("mode", "guest"), ("vmid", "101")]).await;
        assert_eq!(
            (code, out.as_str()),
            (2, "PROXMOX CRITICAL - VM 101 opnsense on pve is stopped")
        );
        let (code, out) = check(&routes, "pve", &[("mode", "guest"), ("vmid", "102")]).await;
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
        let (code, out) = check(&routes, "pve", &[("mode", "backup")]).await;
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
        let (code, out) = check(&routes, "pve", &[("mode", "backup"), ("hours", "168")]).await;
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
        let (code, out) = check(&routes, "pve", &[("mode", "updates")]).await;
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
        let (code, out) = check(&routes, "pve", &[("mode", "updates"), ("node", "pve")]).await;
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
        let (code, out) = check(&routes, "wrong", &[("mode", "cluster")]).await;
        assert_eq!(code, 3, "{out}");
        assert!(out.contains("the API refused the token (401"), "{out}");
        let (code, out) = check(&routes, "pw", &[("mode", "cluster")]).await;
        assert_eq!(
            (code, out.as_str()),
            (
                3,
                "PROXMOX UNKNOWN - secret \"pw\" is a password, not an API token: store it as \
                 one with Replace (Configuration → Secrets → API Token, or watcher secret token pw \
                 --id 'user@realm!name' --replace)"
            )
        );
        let (code, out) = check(&routes, "gone", &[("mode", "cluster")]).await;
        assert_eq!(
            (code, out.as_str()),
            (3, "PROXMOX UNKNOWN - secret \"gone\": no such secret")
        );

        // Another certificate than the pinned one.
        let (port, _) = server(&routes).await;
        let port = port.to_string();
        let other = ["00"; 32].join(":");
        let pool = Pool::new(Arc::new(TestVault));
        let list = [
            ("port", port.as_str()),
            ("secret", "pve"),
            ("mode", "cluster"),
            ("fingerprint", other.as_str()),
        ];
        let (code, out) = Builtin::new("proxmox", "127.0.0.1", &vars(&list))
            .unwrap()
            .run(Duration::from_secs(5), Some(&pool))
            .await;
        assert_eq!(code, 3, "{out}");
        assert!(
            out.starts_with(&format!(
                "PROXMOX UNKNOWN - https://127.0.0.1:{port}: certificate fingerprint is "
            )),
            "{out}"
        );
        // Self-signed, so not trusted by the system.
        let list = [
            ("port", port.as_str()),
            ("secret", "pve"),
            ("mode", "cluster"),
        ];
        let (code, out) = Builtin::new("proxmox", "127.0.0.1", &vars(&list))
            .unwrap()
            .run(Duration::from_secs(5), Some(&pool))
            .await;
        assert_eq!(code, 3, "{out}");
        assert!(
            out.ends_with(
                "invalid peer certificate: UnknownIssuer; for a self-signed certificate, \
                 set vars.fingerprint"
            ),
            "{out}"
        );
        // Without secrets, as in `run-check`.
        let (code, out) = Builtin::new("proxmox", "127.0.0.1", &vars(&list))
            .unwrap()
            .run(Duration::from_secs(5), None)
            .await;
        assert_eq!(code, 3, "{out}");
        assert!(out.contains("secrets aren't available here"), "{out}");
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
