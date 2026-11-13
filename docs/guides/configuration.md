# How the configuration works

Watcher is configured with TOML files, called object files. This page
explains how they are read, how the objects fit together, and which value
wins when the same setting is made in several places. Every field of every
object is listed on the object's own page, see [Objects](#objects).

## Files

Watcher reads every `*.toml` file below the config directory, including
subdirectories. The directory is `conf.d/` next to `watcher.toml` unless
`conf_dir` in [watcher.toml](../reference/watcher-toml.md) says otherwise.
Files and directories whose names start with a dot are skipped.

Any file may contain any kind of object, and the file names have no
meaning. A common layout:

```
conf.d/
  checks.toml      check definitions
  services.toml    service definitions
  hosts.toml       hosts
  groups.toml      host groups, service groups
  contacts.toml    contacts, contact groups
  notify.toml      notify rules, dependencies
  sinks.toml       sinks
  ui/              objects created in the web UI
```

When you save a file, Watcher reloads the configuration within about a
second. If the new configuration has an error, it is not applied: the old
configuration keeps running, and the log and the web UI show the error with
file and line. `SIGHUP` also triggers a reload.

Changes to `watcher.toml` itself need a restart.

## Objects

| Object | Written as | What it is |
|---|---|---|
| [check](../objects/check.md) | `[check.<name>]` | What to run and how to judge the result |
| [service](../objects/service.md) | `[service.<name>]` | A check with its settings, ready to be put on hosts |
| [host](../objects/host.md) | `[[host]]` | A machine or device to monitor |
| [hostgroup](../objects/hostgroup.md) | `[hostgroup.<name>]` | A set of hosts that get the same services and settings |
| [servicegroup](../objects/servicegroup.md) | `[servicegroup.<name>]` | A set of services that are assigned together |
| [contact](../objects/contact.md) | `[contact.<name>]` | Someone who receives notifications |
| [contactgroup](../objects/contactgroup.md) | `[contactgroup.<name>]` | A set of contacts |
| [notify](../objects/notify.md) | `[[notify]]` | A rule: which contacts get which notifications |
| [dependency](../objects/dependency.md) | `[[dependency]]` | Holds notifications back while another host or service is down |
| [downtimegroup](../objects/downtimegroup.md) | `[downtimegroup.<name>]` | Hosts with recurring downtime windows |
| [timeperiod](../objects/timeperiod.md) | `[timeperiod.<name>]` | When notify rules apply, for example business hours |
| [sink](../objects/sink.md) | `[[sink]]` | A second database that receives a copy of all results |

## How a host gets its services

A **check** describes a command. A **service** uses a check and is put on
hosts. A service on a host has the ID `host/service`, for example
`web01/http`. A check can also be put on hosts as it is, without a
service: you only need a `[service.<name>]` for settings of its own, or
for several services from one check.

```
check ─▶ service ─▶ host
            │         ▲
            ▼         │
     servicegroup ─▶ hostgroup
```

There are four ways to put a service on a host:

```toml
# 1. List it on the host
[[host]]
name = "web01"
services = ["http"]

# 2. List a service group on the host
[[host]]
name = "web02"
servicegroups = ["linux-basics"]

# 3. List the service on a host group
[hostgroup.web-servers]
members = ["web*"]
services = ["http"]

# 4. List a service group on a host group
[hostgroup.linux]
members = ["*"]
servicegroups = ["linux-basics"]
```

If a host gets the same service in more than one way, it still has the
service only once.

For a service that only one host needs, you don't need a separate
`[service.<name>]`. Write it directly on the host, see
[Host services](../objects/host.md#host-services).

## Which value wins

The same setting can be made in several places. The more specific place
wins. From most to least specific:

1. the host service (`[host.service.<name>]` on one host)
2. the `for_each` entry (only for `vars`, see
   [for_each](../objects/service.md#one-service-per-item-for_each))
3. the host (only for `vars`)
4. the host's host groups (only for `vars`)
5. the service (`[service.<name>]`)
6. the check (`[check.<name>]`)
7. the built-in default

This applies key by key. A host service that sets only `interval` still
gets its `command`, `vars` and everything else from the service and the
check.

Hosts and host groups can only set `vars` for their services. To change
another setting such as `interval` or `thresholds` on one host, use a host
service.

[Built-in checks](../objects/check.md#built-in-checks) don't take host
and host group vars (3 and 4), so a host var like `port` can't change them
by accident. Use a macro such as `$host.vars.ssh_port$` in the service's
vars to pass one on.

**Tables** like `vars` and `thresholds` are merged key by key: a host that
sets `vars.url` keeps the check's `vars.port`.

**Lists** are replaced, not merged. The exception is `groups`: the service
groups of a service and of a host service add up.

### Host groups that disagree

A host takes `host_check`, `graph`, `ssh` and `vars` from its host groups,
and its own values win over theirs. If two host groups of the same host set
one of these to different values, Watcher can't know which one you mean
and reports an error:

```
hosts.toml:8: error: host "h1": host groups set vars.x differently: a (groups.toml:1) and b (groups.toml:5)
```

Fix it by setting the value on the host, or by removing it from one of the
groups.

## Names

Names of objects:

- must not be empty
- must not contain `/` or control characters
- must not start or end with a space

Names of vars and `for_each` keys use lowercase letters, digits and
underscores, and don't start with a digit: `url`, `disk_root`, `port2`.

## Durations

Durations are written as text: `"30s"`, `"5m"`, `"1h"`, `"1day"`,
`"1h 30m"`.

## Finding out where a value comes from

Check the configuration without starting Watcher:

```sh
watcher config check
watcher config check --services     # also list every service with its command
```

```
OK: 3 files, 1 host, 2 checks, 2 services, 1 on hosts, 1 host check

  web01/http  every 1m     check_http -H 10.0.0.5 -u /
  web01/ping  every 1m     check_ping -H 10.0.0.5 -w '100,20%' -c '500,60%'  (host check)
```

Errors are shown with file and line, and the command exits with code 1:

```
hosts.toml:4: error: host "h1": unknown hostgroup "b"
```

The **Config Tree** shows where an object comes from, where each of its
values is set, and where the object is used:

```sh
watcher config explain web01/http       # a service on a host
watcher config explain web01            # a host
watcher config explain service disk     # a service definition and the hosts that have it
watcher config explain hostgroup linux
```

```
db01/disk_data                                              services.toml:1
├─ from
│  ├─ host db01                                             hosts.toml:6
│  │  └─ vars.disks.data: for_each entry                    hosts.toml:6
│  ├─ service disk                                          services.toml:1
│  │  └─ hostgroup linux: listed                            hosts.toml:1
│  └─ check disk                                            checks.toml:1
│     └─ command: check_disk -w $warn$ -c $crit$ -p $path$
└─ values
   ├─ command: check_disk -w '10%' -c '5%' -p /srv          checks.toml:1
   │  └─ check disk                                         checks.toml:1
   ├─ interval: 5m                                          services.toml:1
   │  └─ service disk                                       services.toml:1
   ...
   └─ vars.warn: 10%                                        hosts.toml:6
      └─ host db01, vars.disks.data                         hosts.toml:6
```

The web UI shows the same tree on every host, service and group page and in
every configuration form.

## Editing in the web UI

The web UI edits the same files. New objects go to `conf.d/ui/`, changes to
existing objects stay in the file where the object is written, and comments
and formatting are kept. See [Web UI](web-ui.md#configuration).

## Editing at the same time

While someone has a file open in the web UI, nobody else can change it there
or through the API. Others see who is editing it and since when, and get the
file read-only. They can still click **Edit Anyway** to take it over.

To edit a file on the server, use `watcher config edit`. It opens the file
in your editor (`$VISUAL`, `$EDITOR`, or `vi`) and locks it in the web UI
until you close the editor:

```sh
watcher config edit hosts.toml
```

If someone has the file open in the web UI, the command tells you who and
stops. `--force` opens it anyway. After you close the editor, it checks the
configuration and shows any errors.

Watcher also notices some editors that you start yourself, because they
leave a lock file next to the file they edit:

| Editor | Lock file | Notes |
|---|---|---|
| vim | `.hosts.toml.swp` | Not with `set directory` pointing elsewhere. |
| nano | `.hosts.toml.swp` | Only with `set locking` in `nanorc`. |
| emacs | `.#hosts.toml` | |
| Kate | `.hosts.toml.kate-swp` | Only while there are unsaved changes. |

Neovim keeps its swap files in its own directory, and VS Code, `sed` and
`sudoedit` leave no lock file, so Watcher can't see them. Copying a file
over with `scp` isn't noticed either. In all these cases nothing is lost
silently: if a file changed on disk after you opened it in the web UI, the
web UI shows the differences before you save (see
[Conflicts](web-ui.md#configuration)).

Lock files of editors that crashed are ignored: Watcher checks whether the
process that wrote the lock file still runs.
