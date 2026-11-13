//! What each built-in check does and which vars it takes, for the web UI's
//! forms and for [`super::vars_of`]. The texts match the tables in
//! `docs/objects/check.md`.

/// What a var takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarKind {
    Text,
    /// A whole number.
    Number,
    /// Seconds (`0.5`) or a duration (`500ms`, `2s`).
    Duration,
    /// `true` or `false`.
    Bool,
    /// One of these values.
    Choice(&'static [&'static str]),
    /// A table, like `headers = { Accept = "..." }`.
    Table,
}

impl VarKind {
    /// `text`, `number`, `duration`, `bool`, `choice` or `table`.
    pub fn as_str(self) -> &'static str {
        match self {
            VarKind::Text => "text",
            VarKind::Number => "number",
            VarKind::Duration => "duration",
            VarKind::Bool => "bool",
            VarKind::Choice(_) => "choice",
            VarKind::Table => "table",
        }
    }

    pub fn choices(self) -> &'static [&'static str] {
        match self {
            VarKind::Choice(list) => list,
            _ => &[],
        }
    }
}

/// One var of a built-in check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VarInfo {
    pub name: &'static str,
    pub help: &'static str,
    /// The default as shown to people, e.g. `the host's address`.
    pub default: Option<&'static str>,
    pub required: bool,
    pub kind: VarKind,
}

/// A built-in check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Info {
    pub name: &'static str,
    pub title: &'static str,
    /// One sentence: what it checks.
    pub summary: &'static str,
    pub vars: &'static [VarInfo],
}

const fn var(name: &'static str, kind: VarKind, help: &'static str) -> VarInfo {
    VarInfo {
        name,
        help,
        default: None,
        required: false,
        kind,
    }
}

const fn default(v: VarInfo, default: &'static str) -> VarInfo {
    VarInfo {
        default: Some(default),
        ..v
    }
}

const fn required(v: VarInfo) -> VarInfo {
    VarInfo {
        required: true,
        ..v
    }
}

const ADDRESS: VarInfo = default(
    var(
        "address",
        VarKind::Text,
        "Connect here instead of the host's address, e.g. for a host that stands for several sites.",
    ),
    "the host's address",
);
const WARN_TIME: VarInfo = var("warn", VarKind::Duration, "WARNING at this response time.");
const CRIT_TIME: VarInfo = var("crit", VarKind::Duration, "CRITICAL at this response time.");

const PVE_ADDRESS: VarInfo = default(
    var(
        "address",
        VarKind::Text,
        "The Proxmox VE API's address, instead of the host's (for a VM or container: of the Proxmox VE host it names).",
    ),
    "the host's address",
);
const PVE_NODE: VarInfo = default(
    var(
        "node",
        VarKind::Text,
        "The node to check. Needed in a cluster with more than one node.",
    ),
    "the only node",
);

/// The built-in checks, in the order of [`super::NAMES`].
pub const INFO: [Info; 11] = [
    Info {
        name: "ping",
        title: "Ping",
        summary: "ICMP echo, like check_ping: round trip time and packet loss.",
        vars: &[
            ADDRESS,
            default(
                var(
                    "packets",
                    VarKind::Number,
                    "Number of ICMP echo requests, sent 200 ms apart.",
                ),
                "5",
            ),
            default(
                var(
                    "warn",
                    VarKind::Text,
                    "WARNING at this average round trip time (ms) or packet loss.",
                ),
                "3000,80%",
            ),
            default(
                var(
                    "crit",
                    VarKind::Text,
                    "CRITICAL at this average round trip time (ms) or packet loss.",
                ),
                "5000,100%",
            ),
        ],
    },
    Info {
        name: "tcp",
        title: "TCP",
        summary: "Connects to a TCP port, optionally sends text and checks the answer.",
        vars: &[
            ADDRESS,
            required(var("port", VarKind::Number, "The port.")),
            var(
                "send",
                VarKind::Text,
                "Text to send after connecting. \\r, \\n, \\t and \\\\ are written as such.",
            ),
            var(
                "expect",
                VarKind::Text,
                "Text the answer must contain, e.g. SSH- or 220. Without it, nothing is read.",
            ),
            WARN_TIME,
            CRIT_TIME,
        ],
    },
    Info {
        name: "http",
        title: "HTTP",
        summary: "Requests a URL, like check_http: status, content and response time.",
        vars: &[
            ADDRESS,
            default(
                var(
                    "url",
                    VarKind::Text,
                    "A full URL (https://www.example.com/login), or a path on the host's address (/health).",
                ),
                "/",
            ),
            default(
                var(
                    "method",
                    VarKind::Choice(&["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"]),
                    "The request method.",
                ),
                "GET",
            ),
            var(
                "headers",
                VarKind::Table,
                "Request headers, e.g. { Accept = \"application/json\" }. Host asks for another site on the same address.",
            ),
            var(
                "body",
                VarKind::Text,
                "Text to send as the request body, e.g. JSON for a POST. Set the matching Content-Type in headers.",
            ),
            var(
                "user",
                VarKind::Text,
                "Log in with HTTP basic auth as this user.",
            ),
            var("password", VarKind::Text, "The password for user."),
            var(
                "status",
                VarKind::Text,
                "Status codes that are OK, e.g. 200 or 200,301-302. Without it: 2xx and 3xx OK, 4xx WARNING, 5xx CRITICAL.",
            ),
            var(
                "content",
                VarKind::Text,
                "Text the body must contain (the first MiB is searched).",
            ),
            default(
                var("follow", VarKind::Bool, "Follow redirects (up to 10)."),
                "false",
            ),
            default(
                var(
                    "verify",
                    VarKind::Bool,
                    "Check the server's certificate. false accepts any certificate.",
                ),
                "true",
            ),
            WARN_TIME,
            CRIT_TIME,
        ],
    },
    Info {
        name: "dns",
        title: "DNS",
        summary: "Looks up a name at a DNS server and checks the answer.",
        vars: &[
            default(
                var(
                    "address",
                    VarKind::Text,
                    "The server to ask, instead of the host's address.",
                ),
                "the host's address",
            ),
            required(var("name", VarKind::Text, "The name to look up.")),
            default(
                var(
                    "type",
                    VarKind::Choice(&["A", "AAAA", "CNAME", "MX", "NS", "PTR", "SOA", "TXT"]),
                    "The record type.",
                ),
                "A",
            ),
            default(
                var(
                    "server",
                    VarKind::Text,
                    "The server to ask. system uses the servers in /etc/resolv.conf.",
                ),
                "the host's address",
            ),
            default(var("port", VarKind::Number, "The server's port."), "53"),
            var(
                "expect",
                VarKind::Text,
                "Values that must all be in the answer, comma-separated: 10.0.0.5, 10.0.0.6.",
            ),
            WARN_TIME,
            CRIT_TIME,
        ],
    },
    Info {
        name: "tls",
        title: "TLS Certificate",
        summary: "Connects with TLS and checks the certificate: trusted, valid for the name, days until it expires.",
        vars: &[
            ADDRESS,
            default(var("port", VarKind::Number, "The port."), "443"),
            default(
                var(
                    "name",
                    VarKind::Text,
                    "The name the certificate must be valid for; also sent to the server (SNI).",
                ),
                "the host's address",
            ),
            default(
                var(
                    "warn",
                    VarKind::Number,
                    "WARNING at this many days before expiry.",
                ),
                "30",
            ),
            default(
                var(
                    "crit",
                    VarKind::Number,
                    "CRITICAL at this many days before expiry.",
                ),
                "14",
            ),
            default(
                var(
                    "verify",
                    VarKind::Bool,
                    "An untrusted certificate, or one not valid for name, gives CRITICAL. false only checks the dates.",
                ),
                "true",
            ),
        ],
    },
    Info {
        name: "proxmox-cluster",
        title: "Proxmox VE Cluster",
        summary: "Quorum, and which nodes are online. Login and certificate from the host's proxmox settings.",
        vars: &[PVE_ADDRESS],
    },
    Info {
        name: "proxmox-node",
        title: "Proxmox VE Node",
        summary: "CPU, memory, swap, load, I/O wait, root file system and uptime of a node; set thresholds on them.",
        vars: &[PVE_ADDRESS, PVE_NODE],
    },
    Info {
        name: "proxmox-storage",
        title: "Proxmox VE Storage",
        summary: "How full each storage is.",
        vars: &[
            PVE_ADDRESS,
            var("node", VarKind::Text, "Only the storages of this node."),
            var(
                "storage",
                VarKind::Text,
                "Only this storage, e.g. local-zfs.",
            ),
            default(
                var("warn", VarKind::Number, "WARNING at this percentage used."),
                "80",
            ),
            default(
                var("crit", VarKind::Number, "CRITICAL at this percentage used."),
                "90",
            ),
        ],
    },
    Info {
        name: "proxmox-guest",
        title: "Proxmox VE Guest",
        summary: "One VM or container: whether it runs, CPU, memory, disk and network.",
        vars: &[
            PVE_ADDRESS,
            default(
                var("vmid", VarKind::Number, "The ID of the VM or container."),
                "the host's proxmox.vmid",
            ),
        ],
    },
    Info {
        name: "proxmox-backup",
        title: "Proxmox VE Backups",
        summary: "Failed or missing backups, and VMs or containers that no backup job includes.",
        vars: &[
            PVE_ADDRESS,
            var("node", VarKind::Text, "Only the backups of this node."),
            default(
                var(
                    "hours",
                    VarKind::Number,
                    "How many hours back to look for backups.",
                ),
                "24",
            ),
        ],
    },
    Info {
        name: "proxmox-updates",
        title: "Proxmox VE Updates",
        summary: "Pending package updates on a node. The token needs Sys.Modify on the node.",
        vars: &[
            PVE_ADDRESS,
            PVE_NODE,
            default(
                var(
                    "warn",
                    VarKind::Number,
                    "WARNING at this many pending updates; 0 never warns.",
                ),
                "1",
            ),
            var(
                "crit",
                VarKind::Number,
                "CRITICAL at this many pending updates.",
            ),
        ],
    },
];

/// The built-in check `name`.
pub fn info(name: &str) -> Option<&'static Info> {
    INFO.iter().find(|i| i.name == name)
}
