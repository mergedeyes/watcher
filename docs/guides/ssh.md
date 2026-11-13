# Checks over SSH

Normally a check runs on the Watcher machine. With `run_on = "host"` it
runs on the monitored host instead, over SSH. Nothing needs to be
installed on the host except the plugins or scripts themselves; there is no
agent. All checks of a host share one SSH connection.

Checks over SSH need a database and a `watcher.toml`, because the keys and
passwords are stored encrypted in the database.

## Setup

**1. Create a key.** Watcher creates and stores it, and prints the public
key:

```sh
watcher secret new monitor
```

```
ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPZ6... watcher-monitor
```

Add this line to `~/.ssh/authorized_keys` of the user on the hosts. In the
web UI: Configuration → Secrets → New Key.

**2. Set the login** on the hosts, usually once on a host group:

```toml
[hostgroup.linux]
members = ["web*", "db*"]
ssh = { user = "monitor", secret = "monitor" }
```

**3. Let checks run on the host:**

```toml
[check.load]
command = "check_load -w 5,4,3 -c 10,8,6"
run_on = "host"
```

To test, use **Run Check** on a service page; it runs over SSH too.

## The ssh field

`ssh` can be set on a [host](../objects/host.md#ssh) and on a
[host group](../objects/hostgroup.md#ssh).

| Field | Default | Description |
|---|---|---|
| `user` | | Required. User to log in as. |
| `secret` | | Required. Name of a key or password stored in Watcher, see [Secrets](#secrets). |
| `port` | `22` | SSH port. |

Watcher connects to the host's `address`. A host can set some fields itself
and take the rest from its host groups:

```toml
[[host]]
name = "db07"
groups = ["linux"]
ssh = { port = 2222 }          # user and secret from linux
```

## Secrets

Keys and passwords are called secrets. Each has a name, which `ssh.secret`
refers to. API tokens for the [proxmox check](proxmox.md) are secrets too;
they are stored the same way, but aren't used for SSH.

| Command | What it does |
|---|---|
| `watcher secret new <name>` | Creates a new ed25519 key and prints the public key. |
| `watcher secret import <name> <file>` | Imports an existing ed25519 or ECDSA key (OpenSSH, PEM or PuTTY format). Asks for the passphrase if the key is encrypted. |
| `watcher secret password <name>` | Stores a password instead of a key. |
| `watcher secret token <name> --id <id>` | Stores an API token: its ID and, asked for, its secret. See [Monitoring Proxmox VE](proxmox.md#3-store-the-token-in-watcher). |
| `watcher secret show <name>` | Prints the public key again. |
| `watcher secret list` | Lists names, kinds, fingerprints of keys and IDs of tokens. |
| `watcher secret remove <name>` | Deletes a secret. |

`--replace` overwrites an existing secret of the same name. Everything
also works in the web UI under Configuration → Secrets.

RSA keys are not supported, neither as login keys nor as host keys. The
Rust `rsa` library has an unfixed timing side channel (RUSTSEC-2023-0071).

Secrets are stored encrypted in the database. The encryption key is the
file `data_dir/secret.key`, created with the first secret (mode 0600).
**Back it up together with the database**: without it, the secrets can't
be decrypted. The same file encrypts the secrets of
[two-factor authentication](web-ui.md#two-factor-authentication).

## Host keys

On the first connection to a host, Watcher stores the host's key (trust on
first use). If the key changes later, for example because the host was
reinstalled, its checks become UNKNOWN until you accept the new key:

```
monitor@db01: host key changed (SHA256:EuEng...); accept: Configuration → Secrets, or watcher hostkey accept db01
```

| Command | What it does |
|---|---|
| `watcher hostkey list` | Lists the stored keys and keys that changed. |
| `watcher hostkey accept <address>[:<port>]` | Accepts the changed key. |
| `watcher hostkey forget <address>[:<port>]` | Deletes the stored key; the next key seen is stored. |

Keys are stored per address and port. SSH host certificates are not
supported.

## How commands run on the host

- The command runs with `sh`, whatever the user's login shell is. sh, bash,
  ksh, zsh and fish work; csh and tcsh only run one-line commands.
- `PATH` is the host's `PATH` plus the usual plugin directories:
  `/usr/lib/monitoring-plugins`, `/usr/lib/nagios/plugins`,
  `/usr/lib64/nagios/plugins`, `/usr/local/libexec/monitoring-plugins`,
  `/usr/local/nagios/libexec`.
- The working directory is the user's home. There is no input and no
  terminal.
- [Macros](../objects/check.md#macros) are replaced on the Watcher machine
  before the command is sent. `$host.address$` is the host's address, as
  for local checks.
- `shell = true` works as it does locally, using the host's `sh`.
- On a timeout the SSH session is closed. The command may keep running on
  the host until it next writes output.

## Copying scripts

With `copy = true`, Watcher uploads the script to the host before it runs,
so you don't have to install it there yourself:

```toml
[check.raid]
command = "check_raid --all"
run_on = "host"
copy = true
```

Watcher takes `check_raid` from its own
[check directories](../objects/check.md#commands), uploads it to
`~/.watcher/checks/check_raid/<hash>` on the host, and runs it from there.
It uploads again when the script changes or was deleted on the host, and
removes older versions.

Things to know:

- Only direct commands, not `shell = true`.
- The host needs `mkdir`, `cat`, `chmod` and `mv`.

## Connections

- Watcher opens one connection per host address, port, user and secret,
  and runs up to 8 checks at a time on it. (OpenSSH allows 10 sessions per
  connection by default.)
- A connection that is unused for 5 minutes is closed.
- If the connection or the login fails, the checks are UNKNOWN with the
  error message. Watcher tries again after 10 seconds.
- Built-in checks and passive services can't run on the host. A check with
  `run_on = "host"` on a host without `ssh` is an error.
