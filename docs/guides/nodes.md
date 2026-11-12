# Nodes

A **node** is a Watcher at another site, for example in a customer's
network or a branch office. It connects to a central Watcher, the **hub**.
The node always dials out to the hub, so the node's site needs no open
ports. The hub needs one port that nodes can reach (7443 by default).

*Not yet:* nodes don't run checks yet. Today a node joins its hub and stays
connected, and the hub shows which nodes are connected. Running the checks
of the node's hosts on the node, keeping results while the link is down,
and the UNREACHABLE state come in the next versions. Until then, the hub
runs every check itself, also those of hosts with a `node`.

## How it works

- You add a node on the hub. The hub makes a **join token**: a one-time
  password that works for 24 hours.
- On the node's machine, `watcher node join` uses the token once. The hub
  gives the node a certificate, and from then on the node logs in with it.
- The node keeps one connection to the hub open and connects again when it
  drops: after 2 seconds, then waiting longer each time, up to a minute.

The hub has its own small certificate authority (CA) for this, in
`data_dir/ca/`. It signs the certificates of the hub and of its nodes. A
node trusts only this CA: the join token contains its fingerprint, so a
node can't be tricked into joining the wrong server. Both sides check each
other's certificate on every connection (mutual TLS).

## Set up a hub

1. Set the role in `watcher.toml`. A hub needs a database: it keeps its
   nodes there.

   ```toml
   role = "hub"

   [database]
   url = "postgres://watcher:secret@localhost/watcher"

   [hub]
   listen = "0.0.0.0:7443"
   url = "https://hub.example.com:7443"
   ```

   `url` is the address nodes use to reach the hub. It goes into the join
   commands. Without it, the hub uses the host of `public_url`, else the
   `listen` address unless it is `0.0.0.0`, else this machine's name; with
   the port of `listen`. See
   [`[hub]`](../reference/watcher-toml.md#hub).
2. Open the port in the firewall for the nodes' addresses.
3. Restart Watcher. The log shows where nodes connect:

   ```
   INFO nodes connect on 0.0.0.0:7443 as https://hub.example.com:7443
   ```

## Add a node

1. On the hub, open **Nodes** (under System) and add the node with a name,
   for example `branch-office`. Names have up to 64 letters, digits, `.`,
   `_` and `-`. Or on the command line:

   ```sh
   watcher node add branch-office
   ```

2. Copy the join command that appears. It looks like this:

   ```
   watcher node join https://hub.example.com:7443 --token wj_…
   ```

3. [Install Watcher](../install.md#build) on the node's machine. It needs
   no database.
4. Run the join command there, as the user that will run the node. Add
   `-c` to choose where `watcher.toml` goes, for example
   `-c /etc/watcher/watcher.toml`. Without it, the command uses
   `./watcher.toml` or `/etc/watcher/watcher.toml`, whichever exists, and
   creates `./watcher.toml` if neither does.

   ```
   Joined the hub at https://hub.example.com:7443 as node branch-office.
   Key and certificates: data/node
   Start the node with: watcher run
   ```

   The command writes the node's key and certificates to `data_dir/node/`
   and sets `role = "node"` and the hub's address in `watcher.toml`.
5. Start the node: `watcher run`, or as a [systemd service](../install.md#systemd).
   The Nodes page on the hub shows it as CONNECTED.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../images/nodes-dark.png">
    <img src="../images/nodes.png" alt="The Nodes page: a join command to copy, and a table of nodes with their state (connected, offline, not joined), the hosts that name them, the machine and the version" width="860">
  </picture>
  <br>
  <sub>The Nodes page on a hub, right after asking for a join command.</sub>
</p>

Hosts name their node with [`node`](../objects/host.md):

```toml
[[host]]
name = "branch-router"
address = "192.0.2.1"
node = "branch-office"
```

A name that hosts use shows up on the Nodes page even before the node is
added, as NOT JOINED, with a **Join Command** button.

## The Nodes page

| Column | Shows |
|---|---|
| State | CONNECTED, OFFLINE (joined, not connected now), or NOT JOINED. |
| Since | How long the node has been connected; when it was last seen; or until when its join token works. |
| Hosts | Hosts in the configuration with this node. |
| Machine | The host name of the node's machine and the address it connected from. |
| Version | The node's Watcher version. |

Everyone who is logged in can see the page. Adding and removing nodes is
for admins.

## Join again

A reinstalled machine, or a node that lost its `data_dir`, joins again
with a new token: **New Join Command** on the Nodes page, or
`watcher node token <name>`. The old certificate keeps working until the
node has joined with the new one; then the hub ends the old connection.

A node that is running picks up the new certificate by itself on its next
connection: there is no need to restart it.

## Remove a node

**Remove** on the Nodes page, or `watcher node remove <name>`. The node's
certificate and its join token stop working at once, and the hub ends its
connection: right away from the web UI, within a minute from the command
line. The node keeps trying and logs why it is refused:

```
WARN the hub refused this node: this node's certificate is no longer valid: …
```

Hosts that name the node keep the name.

## When something goes wrong

| Message | Meaning |
|---|---|
| `the join token is unknown, used or expired` | Each token works once and for 24 hours. Get a new one on the hub. |
| `the hub's certificate is not from the CA the join token names` | The address leads to another server, or the token was cut short when copying. |
| `cannot connect to the hub at …: Connection refused` | Nothing listens there: check `[hub] listen` on the hub, the port and the firewall. |
| `this machine has not joined a hub yet` | Run `watcher node join` first, with the same `watcher.toml`. |
| `role = "hub" needs a database` | Add `[database]` to the hub's `watcher.toml`. |

The hub logs every join, connection and refusal.
