# Checks over SSH

A check with `run_on = "host"` runs on its host, over SSH. Agentless:
plugins or scripts on the host, one connection per host shared by its
checks. Needs a database and `watcher.toml` (for the secrets).

## Setup

1. A key, stored by Watcher:

   ```
   watcher secret new monitor
   ```

   ```
   ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPZ6... watcher-monitor
   ```

   Add the line to `~/.ssh/authorized_keys` of the user on the hosts. In
   the web UI: Configuration → SSH → New Key.

2. Login on the hosts (host or host group):

   ```toml
   [hostgroup.linux]
   members = ["web*", "db*"]
   ssh = { user = "monitor", secret = "monitor" }
   ```

3. Checks that run there:

   ```toml
   [check.load]
   command = "check_load -w 5,4,3 -c 10,8,6"
   run_on = "host"
   ```

Test: Run Check on the service page runs it over SSH as well.

## ssh

| Field | Default | |
|---|---|---|
| `user` | | required |
| `secret` | | required; key or password, see [Secrets](#secrets) |
| `port` | `22` | |

Address: the host's `address`. Host over host group, key by key:

```toml
[[host]]
name = "db07"
groups = ["linux"]
ssh = { port = 2222 }          # user, secret from linux
```

## Secrets

| | |
|---|---|
| `watcher secret new <name>` | new ed25519 key; prints the public key |
| `watcher secret import <name> <file>` | existing ed25519 or ECDSA key: OpenSSH, PEM, PuTTY; passphrase asked if encrypted |
| `watcher secret password <name>` | password |
| `watcher secret show <name>` | public key again |
| `watcher secret list` | names, kinds, fingerprints |
| `watcher secret remove <name>` | |

Or Configuration → SSH. `--replace` overwrites a secret of the same name.

RSA: not supported, neither as key nor as host key (the Rust `rsa` crate
has an unfixed timing side channel, RUSTSEC-2023-0071).

Stored encrypted in the database, with the key in `data_dir/secret.key`
(created with the first secret, mode 0600). Back it up with the database:
without it the secrets can't be decrypted.

## Host keys

The first connection pins the host's key (trust on first use). A
changed key: its checks UNKNOWN until accepted.

```
monitor@db01: host key changed (SHA256:EuEng...); accept: Configuration → SSH, or watcher hostkey accept db01
```

| | |
|---|---|
| `watcher hostkey list` | pinned keys, changed ones |
| `watcher hostkey accept <address>[:<port>]` | pin the changed key |
| `watcher hostkey forget <address>[:<port>]` | the next key is pinned |

Per address and port. Host certificates: not supported.

## Commands on the host

- Run with `sh`, whatever the login shell: sh, bash, ksh, zsh, fish; csh
  and tcsh only one-line commands.
- `PATH`: the host's, plus `/usr/lib/monitoring-plugins`,
  `/usr/lib/nagios/plugins`, `/usr/lib64/nagios/plugins`,
  `/usr/local/libexec/monitoring-plugins`, `/usr/local/nagios/libexec`.
- Working directory: the user's home. No input, no terminal.
- [Macros](checks.md#macros) expand on the Watcher machine
  (`$host.address$` is the host's address, as locally).
- `shell = true`: as locally, with the host's `sh`.
- Timeout: the session is closed; the command may run on until it next
  writes output.

## Copying scripts

```toml
[check.raid]
command = "check_raid --all"
run_on = "host"
copy = true
```

`check_raid` from the [check directories](checks.md#command) is uploaded
to `~/.watcher/checks/check_raid/<hash>` on the host and run from there.
Again after a change, or when deleted on the host; older versions are
removed. Direct commands only, not `shell = true`. Needs `mkdir`, `cat`,
`chmod`, `mv` on the host.

## Connections

- One per host address, port, user and secret; up to 8 checks at a time
  on it (OpenSSH allows 10 sessions per connection by default).
- Unused for 5 minutes: closed.
- Connection or login failed: the checks are UNKNOWN with the error;
  retried after 10 seconds.
