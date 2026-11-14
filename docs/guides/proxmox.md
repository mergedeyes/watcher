# Monitoring Proxmox VE

Watcher checks Proxmox VE through its API, without anything installed on
the Proxmox servers. Six built-in checks report on:

| Check | What it checks |
|---|---|
| `proxmox-cluster` | Quorum, and which nodes are online |
| `proxmox-node` | CPU, memory, swap, load and root file system of a node |
| `proxmox-storage` | How full each storage is |
| `proxmox-guest` | One VM or container: whether it runs, CPU and memory |
| `proxmox-backup` | Failed or missing backups, and guests without a backup job |
| `proxmox-updates` | Pending package updates on a node |

The login is set once, on the host that stands for the Proxmox VE server:
an **API token** (a login for programs that you create in Proxmox VE),
which Watcher stores encrypted like the [secrets for SSH](ssh.md#secrets).
Watcher can also find your VMs and containers (**discovery**) and add
them as hosts with one click.

All options are listed under
[Proxmox VE checks](../objects/check.md#proxmox-ve-checks) and
[proxmox](../objects/host.md#proxmox) on the host page.

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
  nothing. This is enough for every check except `proxmox-updates`, see
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

In the web UI: Configuration → Secrets, choose **API Token**, and enter a
name (for example `pve`), the token ID and the secret.

Or on the Watcher server, next to `watcher.toml`:

```sh
watcher secret token pve --id 'watcher@pve!monitor'
```

It asks for the token's secret (the `value` from step 1). Put the token ID
in single quotes, or the shell reads the `!`.

## 4. Add the Proxmox VE server

Add a host for the server, with the login and the checks you want. In the
web UI: Configuration → Hosts → Add Host → **Proxmox VE**, which opens the
host form with its Proxmox VE section. As a file, for example
`conf.d/proxmox.toml`:

```toml
[[host]]
name = "pve1"
address = "192.0.2.10"
proxmox = { secret = "pve", fingerprint = "18:B4:0D:EC:C4:69:B9:5C:8B:30:C2:82:ED:4A:0C:B9:49:01:0A:3B:04:0F:55:86:68:A8:85:40:6F:22:F3:A2", discover = true }
services = ["proxmox-cluster", "proxmox-node", "proxmox-storage", "proxmox-backup"]
```

- `secret` is the name from step 3, `fingerprint` the one from step 2.
- `discover = true` looks for its VMs and containers, see the next step.
- The checks ask the API at the host's `address` (port 8006). They need no
  `[service.…]`: built-in checks can be listed as they are.

`proxmox-node` only reports values; set [thresholds](../objects/check.md#thresholds)
to be warned, on the host's service:

```toml
  [host.service.proxmox-node]
  thresholds.cpu = { warn = "> 80%", crit = "> 95%" }
  thresholds.memory = { warn = "> 85%", crit = "> 95%" }
```

Then check the file with `watcher config check`. Watcher reloads changed
files on its own; forms in the web UI check before they save.

## 5. Accept VMs and containers

With `discover = true`, Watcher asks the API every 10 minutes for the VMs
and containers (not templates). They are listed under Configuration →
**Discovery**, one list per Proxmox VE host:

| Column | What it shows |
|---|---|
| Name, Type, VM ID, Node | As in Proxmox VE. Type is VM or CT (container). |
| Address | From the QEMU guest agent (VMs) or the container's network, while it runs. The first IPv4 address, else IPv6. VMs without the guest agent have none. |
| Host | The host it was added as, `NEW`, or `ignored`. |

**Accept** adds a guest as a host (**Accept All New** adds all new ones).
**Ignore** stops offering it; **Restore** offers it again. **Remove**
deletes the host again; its history stays in the database.

Accepted guests are written to `conf.d/discovered/<Proxmox VE host>.toml`:

```toml
[[host]]
name = "jellyfin"
address = "192.0.2.71"
services = ["proxmox-guest"]
proxmox = { host = "pve1", vmid = 100 }
```

- The host is named after the guest. If that name is taken, the VM ID is
  added: `jellyfin-100`.
- `proxmox-guest` checks whether it runs, and its CPU and memory. It asks
  the API of `pve1`, with the login from there.
- A guest without an address gets `host_check = "proxmox-guest"`: the API
  says whether it is UP, since there is nothing to ping.
- You can edit these hosts like any other, for example add services or
  host groups. Discovery only keeps `address` and `proxmox` up to date.
- A guest deleted in Proxmox VE is removed from the file on the next run.
  A stopped guest stays, and its `proxmox-guest` check is CRITICAL.

**Run Now** on the Discovery page asks the API right away.

## 6. See that it works

Open the host `pve1` in the web UI. After the first run (within one
interval), each service shows a line like:

```
PROXMOX OK - pve1: CPU 5.2%, memory 23.7% (7.4 GiB of 31.2 GiB), load 0.05, up 3 days
```

To try a service right away, open it and click **Run Check**.

## Clusters

Every node's API answers for the whole cluster. So `proxmox-cluster`,
`proxmox-storage`, `proxmox-backup` and discovery need only one node:
put them on one host, and set `discover = true` only there, or every
node offers the same guests.

`proxmox-node` and `proxmox-updates` check one node: give every node its
own host. Each node has its own certificate, so the fingerprint goes on
each host, and the token on a host group:

```toml
[hostgroup.pve]
members = ["pve*"]
proxmox = { secret = "pve" }
services = ["proxmox-node"]

[[host]]
name = "pve1"
address = "192.0.2.10"
proxmox = { fingerprint = "18:B4:0D:EC:C4:69:B9:5C:8B:30:C2:82:ED:4A:0C:B9:49:01:0A:3B:04:0F:55:86:68:A8:85:40:6F:22:F3:A2", discover = true }
services = ["proxmox-cluster", "proxmox-storage", "proxmox-backup"]

[[host]]
name = "pve2"
address = "192.0.2.11"
proxmox = { fingerprint = "5A:0C:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE" }
```

Each node's `proxmox-node` asks its own address. In a cluster with more
than one node, it also needs to know which node it is about: if the host
names are the node names, add `vars.node = "$host.name$"` to the host
group's service:

```toml
[service.proxmox-node]
vars.node = "$host.name$"
```

## Permissions

`PVEAuditor` reads everything the checks and discovery need, except
`proxmox-updates` (tested with Proxmox VE 9.2).

`proxmox-updates` needs the permission `Sys.Modify` on the node. Proxmox
VE asks for it to list updates, although listing changes nothing. Without
it, the service is UNKNOWN:

```
PROXMOX UNKNOWN - the token lacks a permission: Permission check failed (/nodes/pve1, Sys.Modify)
```

`Sys.Modify` also allows changing the node's network, DNS and time
settings. If you want `proxmox-updates` anyway, give it only this one
permission, on the nodes:

```sh
pveum role add WatcherUpdates --privs Sys.Modify
pveum acl modify /nodes --users watcher@pve --roles WatcherUpdates
```

Proxmox VE looks for updates once a day on its own (`pve-daily-update`),
so the count can be up to a day old.

## When something is wrong

Problems with the API make the service UNKNOWN, with one of these
messages. Discovery shows the same ones on the Discovery page.

| Message | What to do |
|---|---|
| `proxmox-node needs proxmox = { secret = "..." } on the host or a host group` | Set `proxmox` on the host (step 4), or on one of its host groups. |
| `secret "pve": no such secret` | Store the token (step 3), with the name used in `proxmox.secret`. |
| `secret "pve" is a password, not an API token: ...` | The secret was stored as a password. Store it again as an API token, with **Replace** checked (Configuration → Secrets → API Token), or with `watcher secret token pve --id ... --replace`. |
| `the API refused the token (401 ...)` | The token ID or secret is wrong, or the token was deleted or has expired. |
| `the token lacks a permission: ...` | Give the user the role from step 1, or see [Permissions](#permissions). |
| `certificate fingerprint is ..., not the one in proxmox.fingerprint` | The certificate changed (renewed, or replaced). Read the new fingerprint on the node (step 2) and put it into `proxmox.fingerprint`. The message shows the fingerprint the server sent; check it on the node before you use it, or you'd trust whoever answered. |
| `invalid peer certificate: UnknownIssuer; for a self-signed certificate, set proxmox.fingerprint on the host` | The certificate is self-signed and `fingerprint` isn't set. Add it (step 2). |
| `2 nodes (pve1, pve2): set vars.node` | In a cluster, `proxmox-node` and `proxmox-updates` need `vars.node`. |
| `no complete answer within ...` | The API didn't answer in time. Is the address right, and is port 8006 open from Watcher? |
