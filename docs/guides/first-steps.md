# First steps

This guide goes from an empty directory to a monitored host that sends
email alerts. You need the
[monitoring plugins](https://www.monitoring-plugins.org) (for example
`apt install monitoring-plugins`) and a database, see
[Installation](../install.md).

## 1. Set up Watcher

```sh
mkdir ~/watcher && cd ~/watcher
watcher init
watcher run
```

`watcher init` asks a few questions and writes `watcher.toml` and the
`conf.d/` directory. Open http://127.0.0.1:8080 and log in with the admin
account you created.

## 2. Write a check

A [check](../objects/check.md) says what to run. Create
`conf.d/checks.toml`:

```toml
[check.http]
command = "check_http -H $host.address$ -u $url$"
vars.url = "/"
thresholds.time = { warn = "> 500ms", crit = "> 2s" }
```

`$host.address$` will be the address of the host, and `$url$` the var
`url`. The threshold turns the state WARNING when the page takes longer
than 500 ms, and CRITICAL above 2 seconds.

You can try a command before you configure it:

```sh
watcher run-check -- check_http -H example.com -u /
```

**No plugins installed?** Leave out `checks.toml`. Watcher has a
[built-in `http` check](../objects/check.md#http), and the host in the
next step uses it automatically. It takes the path from `vars.url` and
time limits from `vars.warn` and `vars.crit` (for example `"500ms"` and
`"2s"`), set on the host's service (see
[Host services](../objects/host.md#host-services)).

## 3. Add a host

Create `conf.d/hosts.toml`:

```toml
[[host]]
name = "web01"
address = "10.0.0.5"
services = ["http"]
```

`services = ["http"]` puts the check `http` on the host. On the host it is
a [service](../objects/service.md) with the ID `web01/http`. You only
write a `[service.<name>]` when a service needs settings of its own, or to
make several services from one check, for example one per URL.

Watcher loads saved files within a second. The log shows:

```
reloading (file change)
```

Because the host has an address, Watcher also pings it to decide whether
it is UP or DOWN. See [Host state](../objects/host.md#host-state).

## 4. Check the result

```sh
watcher config check --services
```

```
OK: 2 files, 1 host, 1 check, 2 on hosts

  web01/http  every 1m     check_http -H 10.0.0.5 -u /
  web01/ping  every 1m     ping -H 10.0.0.5 -p 5 -w 3000,80% -c 5000,100% (built-in)
```

In the web UI, open Hosts → web01. The Config Tree on that page shows
where each value comes from.

## 5. Get alerts

Create `conf.d/contacts.toml` with a [contact](../objects/contact.md) and a
[notify rule](../objects/notify.md):

```toml
[contact.oncall]
email = "oncall@example.com"

[[notify]]
contacts = ["oncall"]
```

Email needs an SMTP server in `watcher.toml`, see
[Notifications](notifications.md#smtp). Then send a test message:

```sh
watcher notify test oncall
```

## 6. The same in the web UI

Everything above also works under Configuration → Checks and Hosts → Add.
Each save shows what will change (+ ~ −) before you click Apply. The web
UI writes to the same files.

## Next

- [How the configuration works](configuration.md): how objects fit
  together and which value wins
- [hostgroup](../objects/hostgroup.md): give many hosts the same services
- [for_each](../objects/service.md#one-service-per-item-for_each): one
  service per disk, website or queue
- [Examples](../examples.md): complete setups
