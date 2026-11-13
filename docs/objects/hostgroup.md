# hostgroup

A host group is a set of hosts that get the same services and settings.
Instead of listing `http` on every web server, you list it once on the
group. Host groups also group hosts on the status pages, in filters and in
[notify rules](notify.md).

```toml
[hostgroup.web-servers]
title = "Web Servers"
members = ["web*"]
services = ["http", "https_cert"]
```

Every host whose name starts with `web` is in the group and gets the
services `http` and `https_cert`.

## Fields

| Field | Default | Description |
|---|---|---|
| `title` | | Name shown in the web UI. |
| `members` | | Host names. `*` matches any text, `?` one character. |
| `services` | | Services every member gets. A [check](check.md) can be listed as it is, without a `[service.<name>]`: a [built-in](check.md#built-in-checks) one like `http` or `tls`, or one of yours. |
| `servicegroups` | | Service groups every member gets, with all their services. |
| `host_check` | | The service or check that decides whether its hosts are UP or DOWN. It runs for each host itself and isn't one of its services. `false`: no host check. See [Host state](host.md#host-state). |
| `graph` | | The service whose graph is shown on the host pages. A host without that service, or with its graphs off, shows its host check's graph. |
| `ssh` | | Login for [checks over SSH](../guides/ssh.md): `{ user, secret, port }`. See [ssh](#ssh). |
| `proxmox` | | The Proxmox VE API login for its hosts: `{ secret, fingerprint, port, verify, discover }`. See [proxmox](#proxmox). |
| `vars` | | Default vars for its hosts, used by `$name$` macros and `for_each`. [Built-in checks](check.md#built-in-checks) don't read them, except through macros like `$host.vars.port$`. |

A host's own `host_check`, `graph`, `ssh`, `proxmox` and `vars` win over its host
groups'. If two host groups of the same host set one of them to different
values, that is an error; see
[Host groups that disagree](../guides/configuration.md#host-groups-that-disagree).

## Members

A host joins a host group in either of two ways, and both can be mixed:

```toml
# On the group, by name or pattern
[hostgroup.databases]
members = ["db*", "pg-main"]

# On the host
[[host]]
name = "reporting01"
groups = ["databases"]
```

The web UI shows the full membership on both the host and the group,
including where each membership is written.

## Examples

### Every host

```toml
[hostgroup.all]
members = ["*"]
services = ["load"]
```

### A group that only groups

A host group without services is still useful for status pages and notify
rules:

```toml
[hostgroup.prod]
members = ["web01", "web02", "db01"]
```

### Services and service groups for the members

```toml
[hostgroup.linux]
members = ["*"]
services = ["ssh"]
servicegroups = ["linux-basics"]
```

### Host check for the members

```toml
[hostgroup.switches]
members = ["sw-*"]
host_check = "snmp_uptime"
```

### Default vars

Every member checks `/` with `for_each = "disks"`. Members can add more
disks with their own `vars.disks`.

```toml
[hostgroup.linux]
members = ["*"]
services = ["disk"]
vars.disks.root = { path = "/" }
```

### SSH login for the members

```toml
[hostgroup.linux]
members = ["web*", "db*"]
ssh = { user = "monitor", secret = "monitor" }
```

## ssh

| Field | Default | Description |
|---|---|---|
| `user` | | User to log in as. A host needs it from itself or a host group. |
| `secret` | | Name of a key or password stored in Watcher. A host needs it from itself or a host group. See [Secrets](../guides/ssh.md#secrets). |
| `port` | `22` | SSH port. |

A host can set some of these itself, for example only `port`, and take the
others from the group.

## proxmox

The login of the [Proxmox VE checks](check.md#proxmox-ve-checks), for
hosts that are Proxmox VE servers.

| Field | Default | Description |
|---|---|---|
| `secret` | | Name of the API token stored in Watcher. A Proxmox VE server needs it from itself or a host group. See [Store the token](../guides/proxmox.md#3-store-the-token-in-watcher). |
| `fingerprint` | | The SHA-256 fingerprint of the API's certificate. Each node has its own certificate, so it usually goes on the host. |
| `port` | `8006` | The API's port. |
| `verify` | `true` | Without `fingerprint`: the certificate must be trusted by the system. `false` accepts any certificate. |
| `discover` | `false` | Offer the VMs and containers as hosts. In a cluster, set it on one node only: every node lists all guests. |

A host can set some of these itself, for example only `fingerprint`, and
take the others from the group. VMs and containers (`host`, `vmid`) are
set on the host, see [proxmox on hosts](host.md#proxmox).

## Checking members

```sh
watcher config explain hostgroup linux
```

The output lists the members and why each one is a member (listed on the
host, or matched by a pattern).

In the web UI, Current Status → Host Groups shows the status totals of each
group, linked to the filtered host and service lists.

## See also

- [host](host.md)
- [servicegroup](servicegroup.md)
- [How a host gets its services](../guides/configuration.md#how-a-host-gets-its-services)
