# Monitoring Proxmox VE

Watcher checks Proxmox VE through its API, without anything installed on
the Proxmox servers. The built-in check `proxmox` reports on:

- the **cluster**: quorum and which nodes are online,
- each **node**: CPU, memory, swap, load, root file system,
- the **storages**: how full they are,
- **VMs and containers** (guests): whether they run, CPU and memory,
- **backups**: failed backups, missing backups, guests without a backup job,
- **updates**: pending package updates on a node.

It logs in with an **API token**: a login for programs that you create in
Proxmox VE. Watcher stores the token encrypted, like the
[secrets for SSH](ssh.md#secrets).

All vars and metrics are listed under
[proxmox](../objects/check.md#proxmox) on the check page.

## 1. Create a user and an API token in Proxmox VE

Run this on a Proxmox VE node, as root:

```sh
pveum user add watcher@pve --comment "Watcher monitoring"
pveum acl modify / --users watcher@pve --roles PVEAuditor
pveum user token add watcher@pve monitor --privsep 0
```

- The first line creates the user `watcher` in the Proxmox VE realm `pve`.
  It needs no password, since it only logs in with the token.
- `PVEAuditor` is a built-in role that can read everything but change
  nothing. This is enough for every mode except `updates`, see
  [Permissions](#permissions).
- The last line creates the token `monitor`. `--privsep 0` gives it the
  same rights as its user.

The last command prints the token. Keep the window open for step 3:

```
┌──────────────┬──────────────────────────────────────┐
│ key          │ value                                │
╞══════════════╪══════════════════════════════════════╡
│ full-tokenid │ watcher@pve!monitor                  │
│ value        │ 5f3c2a9e-0d4b-4c41-9a0e-7f1d2b3c4d5e │
└──────────────┴──────────────────────────────────────┘
```

`full-tokenid` is the token ID, `value` is its secret. Proxmox VE shows the
secret only this once.

You can do the same in the Proxmox VE web interface under Datacenter →
Permissions: add the user under Users, give it the role on `/` under
Permissions, and add the token under API Tokens (uncheck "Privilege
Separation").

## 2. Find the certificate fingerprint

A new Proxmox VE server uses a self-signed certificate, which Watcher
doesn't trust on its own. Tell Watcher which certificate to expect by its
fingerprint. On the node:

```sh
openssl x509 -in /etc/pve/local/pve-ssl.pem -noout -fingerprint -sha256
```

```
sha256 Fingerprint=18:B4:0D:EC:C4:69:B9:5C:8B:30:C2:82:ED:4A:0C:B9:49:01:0A:3B:04:0F:55:86:68:A8:85:40:6F:22:F3:A2
```

The web interface shows the same value under the node → System →
Certificates → `pve-ssl.pem` → View Certificate.

If you replaced the certificate (your own one in `pveproxy-ssl.pem`, or one
from Let's Encrypt through ACME), use that file instead. A certificate from
Let's Encrypt is trusted anyway: then leave out `fingerprint`.

## 3. Store the token in Watcher

On the Watcher server, next to `watcher.toml`:

```sh
watcher secret token pve --id 'watcher@pve!monitor'
```

It asks for the token's secret (the `value` from step 1). `pve` is the
name you give the secret in Watcher; the services use it in
`vars.secret`. Put the token ID in single quotes, or the shell reads the
`!`.

In the web UI: Configuration → Secrets, choose **API Token**, and enter the
name, the token ID and the secret.

## 4. Add the services

Put this into a file in `conf.d/`, for example `conf.d/proxmox.toml`, and
change the address, the fingerprint and the VM ID:

```toml
# The settings all Proxmox services share.
[check.pve]
builtin = "proxmox"
vars = { secret = "pve", fingerprint = "18:B4:0D:EC:C4:69:B9:5C:8B:30:C2:82:ED:4A:0C:B9:49:01:0A:3B:04:0F:55:86:68:A8:85:40:6F:22:F3:A2" }

[service.pve-cluster]
check = "pve"
vars.mode = "cluster"

[service.pve-node]
check = "pve"
vars.mode = "node"
thresholds.cpu = { warn = "> 80%", crit = "> 95%" }
thresholds.memory = { warn = "> 85%", crit = "> 95%" }

[service.pve-storage]
check = "pve"
vars.mode = "storage"

[service.pve-backup]
check = "pve"
vars.mode = "backup"

[service.pve-jellyfin]
check = "pve"
vars = { mode = "guest", vmid = "100" }

[[host]]
name = "pve1"
address = "192.0.2.10"
services = ["pve-cluster", "pve-node", "pve-storage", "pve-backup", "pve-jellyfin"]
```

Services take the check's vars and add their own, so `secret` and
`fingerprint` are set once in `[check.pve]`. The `node` mode doesn't change
the state on its own (only when the node is offline): the `thresholds` decide
when CPU or memory are too high.

Then check the file and reload:

```sh
watcher config check
```

Watcher reloads changed files on its own. When you edit in the web UI,
it checks the file before saving.

## 5. See that it works

Open the host `pve1` in the web UI. After the first run (within one
interval), each service shows a line like:

```
PROXMOX OK - pve1: CPU 5.2%, memory 23.7% (7.4 GiB of 31.2 GiB), load 0.05, up 3 days
```

To try a service right away, open it and click **Run Check**.

## Clusters

Every node's API answers for the whole cluster. So `cluster`, `storage`,
`backup` and `guest` need only one address. For `node` and `updates`, set
which node to check: `vars.node = "pve2"`.

If you have a host for each node and the host names are the node names,
let a host group give every node its services. Change `[service.pve-node]`
from step 4 to this, and add the host group:

```toml
[service.pve-node]
check = "pve"
vars = { mode = "node", node = "$host.name$", fingerprint = "$host.vars.pve_fingerprint$" }

[hostgroup.proxmox]
members = ["pve*"]
services = ["pve-node"]

[[host]]
name = "pve2"
address = "192.0.2.11"
vars.pve_fingerprint = "5A:0C:…"     # step 2, on pve2
```

Each node is then asked about itself, at its own address. Every node has
its own certificate, so each host carries its fingerprint in a host var:
add `vars.pve_fingerprint` to `pve1` from step 4 too.
Built-in checks don't read host vars directly; the macro
`$host.vars.pve_fingerprint$` brings it in.

## VMs and containers as hosts

A guest can be its own host in Watcher, with ping and other checks, and a
`guest` service that asks the Proxmox API about it. Since the API is on
the Proxmox server and not on the guest, set the service's `address`:

```toml
[service.pve-guest]
check = "pve"
vars = { mode = "guest", address = "192.0.2.10" }

[[host]]
name = "jellyfin"
address = "192.0.2.21"             # the host check pings this address
  [host.service.pve-guest]
  vars.vmid = "100"
```

*Not yet:* Watcher finding VMs and containers on its own and offering
them as new hosts.

## Permissions

`PVEAuditor` reads everything the modes `cluster`, `node`, `storage`,
`guest` and `backup` need.

The `updates` mode needs the permission `Sys.Modify` on the node. Proxmox
VE asks for it to list updates, although listing changes nothing. Without
it, the service is UNKNOWN:

```
PROXMOX UNKNOWN - the token lacks a permission: Permission check failed (/nodes/pve1, Sys.Modify)
```

`Sys.Modify` also allows changing the node's network, DNS and time
settings. If you want the `updates` mode anyway, give it only this one
permission, on the nodes:

```sh
pveum role add WatcherUpdates --privs Sys.Modify
pveum acl modify /nodes --users watcher@pve --roles WatcherUpdates
```

Proxmox VE looks for updates once a day on its own (`pve-daily-update`),
so the count can be up to a day old.

## When something is wrong

API problems make the service UNKNOWN with one of these messages:

| Message | What to do |
|---|---|
| `secret "pve": no such secret` | Store the token (step 3), with the name used in `vars.secret`. |
| `secret "pve" is a password, not an API token: ...` | The secret was stored as a password. Store it again as an API token, with **Replace** checked (Configuration → Secrets → API Token), or with `watcher secret token pve --id ... --replace`. |
| `the API refused the token (401 ...)` | The token ID or secret is wrong, or the token was deleted or has expired. |
| `the token lacks a permission: ...` | Give the user the role from step 1, or see [Permissions](#permissions). |
| `certificate fingerprint is ..., not the one in vars.fingerprint` | The certificate changed (renewed, or replaced). Read the new fingerprint on the node (step 2) and put it into `vars.fingerprint`. The message shows the fingerprint the server sent; check it on the node before you use it, or you'd trust whoever answered. |
| `invalid peer certificate: UnknownIssuer; for a self-signed certificate, set vars.fingerprint` | The certificate is self-signed and `fingerprint` isn't set. Add it (step 2). |
| `2 nodes (pve1, pve2): set vars.node` | In a cluster, `node` and `updates` need `vars.node`. |
| `no complete answer within ...` | The API didn't answer in time. Is the address right, and is port 8006 open from Watcher? |
