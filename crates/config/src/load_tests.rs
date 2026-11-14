use super::*;
use crate::{Alert, state_name};
use watcher_core::State;

/// Compiles in-memory files given as `(path, text)`.
fn compile(files: &[(&str, &str)]) -> (Config, Vec<Diagnostic>) {
    let mut sink = Sink::default();
    let mut raw = RawObjects::default();
    for (path, text) in files {
        raw.add_file(Path::new(path), text, &mut sink);
    }
    let config = raw.compile(&mut sink);
    (config, sink.finish())
}

fn messages(diags: &[Diagnostic]) -> Vec<String> {
    diags.iter().map(|d| d.to_string()).collect()
}

fn ok(files: &[(&str, &str)]) -> Config {
    let (config, diags) = compile(files);
    let errors: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.to_string())
        .collect();
    assert_eq!(errors, Vec::<String>::new());
    config
}

fn argv(service: &Service) -> Vec<&str> {
    match &service.command {
        Some(CommandLine::Argv(a)) => a.iter().map(String::as_str).collect(),
        other => panic!("{other:?}"),
    }
}

const CHECKS: &str = r#"
[check.ping]
command = "check_ping -H $host.address$ -w $warn$ -c $crit$"
vars = { warn = "100,20%", crit = "500,60%" }

[check.load]
command = "check_load -w 5,4,3 -c 10,8,6"

[check.disk]
command = "check_disk -w $warn$ -c $crit$ -p $path$"
vars = { warn = "20%", crit = "10%" }

[check.http]
command = "check_http -H $host.address$ -u $url$"
vars.url = "/"
thresholds.time = { warn = "> 500ms", crit = "> 2s" }
"#;

const SERVICES: &str = r#"
[service.ping]

[service.load]
check = "load"

[service.disk]
check = "disk"
for_each = "disks"
groups = ["storage"]

[service.http]
check = "http"
interval = "2m"

[service.backup]
passive = true
freshness = "26h"
"#;

const HOSTS: &str = r#"
[[host]]
name = "web01"
address = "10.0.0.5"
groups = ["web"]
vars.disks = { root = { path = "/" } }

[[host]]
name = "db01"
groups = ["dc1"]
services = ["backup"]
vars.disks.root = { path = "/" }
vars.disks.pgdata = { path = "/var/lib/postgresql", warn = "10%", crit = "5%" }

  [host.service.load]
  enabled = false
"#;

const GROUPS: &str = r#"
[hostgroup.linux]
members = ["*"]
host_check = "ping"
servicegroups = ["basics"]

[hostgroup.web]
services = ["http"]

[hostgroup.dc1]
title = "Datacenter 1"

[servicegroup.basics]
services = ["ping", "load", "disk"]

[servicegroup.storage]

[servicegroup.webshop]
title = "Webshop"
services = ["http", "backup"]

[contactgroup.dba]
members = ["alice", "bob"]

[contact.oncall]
email = "oncall@example.com"

[contact.alice]
email = "alice@example.com"

[contact.bob]
email = "bob@example.com"
groups = ["dba"]

[[notify]]
contacts = ["oncall"]
hosts = { groups = ["dc1", "web"] }

[[notify]]
name = "dba-critical"
contact_groups = ["dba"]
services = { groups = ["webshop"] }
states = ["critical"]
"#;

fn files() -> [(&'static str, &'static str); 4] {
    [
        ("checks.toml", CHECKS),
        ("services.toml", SERVICES),
        ("hosts.toml", HOSTS),
        ("groups.toml", GROUPS),
    ]
}

fn example() -> Config {
    ok(&files())
}

fn names(list: &[Assigned]) -> Vec<&str> {
    list.iter().map(|a| a.name.as_str()).collect()
}

#[test]
fn compiles_the_design_example() {
    let (_, diags) = compile(&files());
    assert_eq!(messages(&diags), Vec::<String>::new());
    let config = example();
    let ids: Vec<_> = config.services.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "web01/disk_root",
            "web01/http",
            "web01/load",
            "web01/ping",
            "db01/backup",
            "db01/disk_pgdata",
            "db01/disk_root",
            "db01/ping",
        ]
    );

    let pgdata = config.service("db01/disk_pgdata").unwrap();
    assert_eq!(
        argv(pgdata),
        [
            "check_disk",
            "-w",
            "10%",
            "-c",
            "5%",
            "-p",
            "/var/lib/postgresql"
        ]
    );
    assert_eq!(pgdata.settings.interval, Duration::from_secs(60));
    assert_eq!(pgdata.check.as_deref(), Some("disk"));
    assert_eq!(pgdata.object.as_deref(), Some("disk"));
    assert_eq!(
        pgdata.for_each,
        Some(("disks".to_string(), "pgdata".to_string()))
    );
    // Where values came from.
    assert_eq!(pgdata.sources["vars.warn"].layer, Layer::ForEach);
    assert_eq!(pgdata.sources["command"].layer, Layer::Check);
    assert_eq!(pgdata.origin.line, 7);
    assert_eq!(names(&pgdata.groups), ["basics", "storage"]);
    assert!(matches!(
        &pgdata.assigned[0].reason,
        Reason::Listed { by, .. } if by == "servicegroup basics on hostgroup linux"
    ));

    let root = config.service("web01/disk_root").unwrap();
    assert_eq!(
        argv(root),
        ["check_disk", "-w", "20%", "-c", "10%", "-p", "/"]
    );

    let http = config.service("web01/http").unwrap();
    assert_eq!(argv(http), ["check_http", "-H", "10.0.0.5", "-u", "/"]);
    assert!(http.settings.thresholds["time"].warn.is_some());
    assert_eq!(http.settings.interval, Duration::from_secs(120));
    assert_eq!(http.sources["interval"].layer, Layer::Service);
    assert_eq!(names(&http.groups), ["webshop"]);

    let backup = config.service("db01/backup").unwrap();
    assert!(backup.settings.passive);
    assert_eq!(backup.settings.max_attempts, 1);
    assert!(matches!(
        &backup.assigned[0].reason,
        Reason::Listed { by, .. } if by == "host db01"
    ));

    // Host groups: listed on the host, or by the group's members.
    let web = &config.hosts[0];
    assert_eq!(names(&web.groups), ["web", "linux"]);
    assert!(matches!(
        &web.groups[1].reason,
        Reason::Selector { matched, .. } if matched == "members *"
    ));
    assert_eq!(web.host_check.as_deref(), Some("ping"));
    assert_eq!(web.sources["host_check"].layer, Layer::HostGroup);
    let db = &config.hosts[1];
    assert_eq!(names(&db.groups), ["dc1", "linux"]);
    assert_eq!(config.host_check(db).unwrap().id, "db01/ping");
    assert!(config.service("db01/load").is_none());
    let (i, named) = config.host_named("db01").unwrap();
    assert_eq!(named.name, "db01");
    let own: Vec<_> = config.services_of(i).map(|s| s.id.as_str()).collect();
    let all: Vec<_> = (config.services.iter())
        .filter(|s| config.host(s).name == "db01")
        .map(|s| s.id.as_str())
        .collect();
    assert_eq!(own, all);
    assert!(config.host_named("nope").is_none());

    let dba: Vec<_> = config
        .group_contacts("dba")
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(dba, ["alice", "bob"]);
}

#[test]
fn notify_rules_add_up() {
    let config = example();
    let names = |id: &str, alert, state| -> Vec<String> {
        let s = config.service(id).unwrap();
        let host_alert = config.is_host_check(s);
        config
            .recipients(s, host_alert, alert, state, None)
            .iter()
            .map(|c| c.name.clone())
            .collect()
    };
    assert_eq!(
        names("db01/backup", Alert::Problem, "critical"),
        ["oncall", "alice", "bob"]
    );
    assert_eq!(names("db01/backup", Alert::Problem, "warning"), ["oncall"]);
    // A recovery from WARNING doesn't reach the critical-only rule.
    assert_eq!(names("db01/backup", Alert::Recovery, "warning"), ["oncall"]);
    assert_eq!(
        names("web01/http", Alert::Problem, "critical"),
        ["oncall", "alice", "bob"]
    );
    // The host check alerts as the host: rules with services don't apply.
    assert_eq!(names("db01/ping", Alert::Problem, "down"), ["oncall"]);
    assert_eq!(state_name(State::Critical, true), "down");
}

#[test]
fn notify_repeat() {
    let base = r#"
[contact.ops]
[contact.boss]

[[host]]
name = "a"
  [host.service.disk]
  command = "true"
"#;
    let config = ok(&[(
        "a.toml",
        &format!(
            r#"{base}
[[notify]]
contacts = ["ops", "boss"]
repeat = "4h"

[[notify]]
contacts = ["ops"]
states = ["critical"]
repeat = "30m"

[[notify]]
contacts = ["boss"]
repeat = "0"
"#
        ),
    )]);
    assert_eq!(config.notify[2].repeat, None);
    let s = config.service("a/disk").unwrap();
    let every = |state| -> Vec<(String, u64)> {
        config
            .repeat_recipients(s, false, state)
            .into_iter()
            .map(|(c, d)| (c.name.clone(), d.as_secs() / 60))
            .collect()
    };
    // The shortest interval of the rules that take the state.
    assert_eq!(
        every("critical"),
        [("ops".into(), 30), ("boss".into(), 240)]
    );
    assert_eq!(
        every("warning"),
        [("ops".into(), 240), ("boss".into(), 240)]
    );

    let (_, diags) = compile(&[(
        "a.toml",
        &format!(
            r#"{base}
[[notify]]
contacts = ["ops"]
repeat = "30s"

[[notify]]
contacts = ["ops"]
on = ["recovery"]
repeat = "1h"

[[notify]]
contacts = ["ops"]
repeat = "soon"
"#
        ),
    )]);
    let text = messages(&diags).join("\n");
    assert!(text.contains("repeat: must be at least 1m"), "{text}");
    assert!(text.contains("on doesn't include problem"), "{text}");
    assert!(text.contains("invalid duration \"soon\""), "{text}");
}

#[test]
fn notify_after_escalates() {
    let base = r#"
[contact.ops]
[contact.boss]

[[host]]
name = "a"
  [host.service.disk]
  command = "true"
"#;
    let config = ok(&[(
        "a.toml",
        &format!(
            r#"{base}
[[notify]]
contacts = ["ops"]
repeat = "4h"

[[notify]]
contacts = ["boss"]
states = ["critical"]
after = "30m"
repeat = "1h"

[[notify]]
contacts = ["ops"]
after = "0"
"#
        ),
    )]);
    assert_eq!(config.notify[1].after, Some(Duration::from_secs(1800)));
    assert_eq!(config.notify[2].after, None);
    let s = config.service("a/disk").unwrap();
    let names =
        |cs: Vec<&crate::Contact>| -> Vec<String> { cs.iter().map(|c| c.name.clone()).collect() };
    // Not told at once, and their repeat isn't a plain repeat.
    assert_eq!(
        names(config.recipients(s, false, Alert::Problem, "critical", None)),
        ["ops"]
    );
    assert_eq!(config.repeat_recipients(s, false, "critical").len(), 1);
    let esc: Vec<(String, u64, Option<u64>)> = config
        .followups(s, false, "critical")
        .into_iter()
        .map(|f| {
            (
                f.contact.name.clone(),
                f.after.unwrap().as_secs() / 60,
                f.repeat.map(|r| r.as_secs() / 60),
            )
        })
        .collect();
    assert_eq!(esc, [("boss".into(), 30, Some(60))]);
    assert!(config.followups(s, false, "warning").is_empty());
    assert_eq!(
        names(config.followup_recoveries(s, false, "critical")),
        ["boss"]
    );
    // A host without host check: escalations get no downtime messages.
    let (_, host) = config.host_named("a").unwrap();
    assert_eq!(names(config.host_downtime_recipients(host, None)), ["ops"]);

    let (_, diags) = compile(&[(
        "a.toml",
        &format!(
            r#"{base}
[[notify]]
contacts = ["ops"]
on = ["recovery"]
after = "1h"

[[notify]]
contacts = ["ops"]
on = ["problem", "flapping"]
after = "1h"
"#
        ),
    )]);
    let text = messages(&diags).join("\n");
    assert!(
        text.contains("after: escalates problems, but on doesn't include problem"),
        "{text}"
    );
    assert!(
        text.contains("after: escalations take only problem and recovery"),
        "{text}"
    );
}

#[test]
fn precedence_host_over_group_over_service_over_check() {
    let config = ok(&[(
        "a.toml",
        r#"
[check.http]
command = "check_http -u $url$ -t $timeout_s$ -p $port$"
vars = { url = "/", timeout_s = "10", port = "80" }
interval = "5m"
timeout = "20s"

[service.http]
interval = "2m"
vars.url = "/status"
timeout = "30s"

[hostgroup.web]
members = ["a", "b"]
services = ["http"]
vars.port = "8080"

[[host]]
name = "a"

[[host]]
name = "b"
vars.url = "/health"
  [host.service.http]
  vars.timeout_s = "5"
  interval = "10s"
"#,
    )]);
    let a = config.service("a/http").unwrap();
    assert_eq!(
        argv(a),
        ["check_http", "-u", "/status", "-t", "10", "-p", "8080"]
    );
    assert_eq!(a.settings.interval, Duration::from_secs(120));
    assert_eq!(a.settings.timeout, Duration::from_secs(30));
    assert_eq!(a.sources["vars.port"].layer, Layer::HostGroup);
    let b = config.service("b/http").unwrap();
    // Host vars beat the service; the host's service beats all.
    assert_eq!(
        argv(b),
        ["check_http", "-u", "/health", "-t", "5", "-p", "8080"]
    );
    assert_eq!(b.settings.interval, Duration::from_secs(10));
    assert_eq!(b.sources["interval"].layer, Layer::HostService);
}

#[test]
fn services_come_from_every_path_once() {
    let config = ok(&[(
        "a.toml",
        r#"
[check.load]
command = "check_load"

[service.load]

[servicegroup.basics]
services = ["load"]

[hostgroup.linux]
members = ["h"]
services = ["load"]
servicegroups = ["basics"]

[[host]]
name = "h"
services = ["load"]
servicegroups = ["basics"]
"#,
    )]);
    assert_eq!(config.services.len(), 1);
    let by: Vec<String> = config.services[0]
        .assigned
        .iter()
        .map(|a| match &a.reason {
            Reason::Listed { by, .. } => by.clone(),
            Reason::Selector { matched, .. } => matched.clone(),
        })
        .collect();
    assert_eq!(
        by,
        [
            "host h",
            "servicegroup basics on host h",
            "hostgroup linux",
            "servicegroup basics on hostgroup linux"
        ]
    );
}

#[test]
fn a_host_service_named_like_a_service_assigns_it() {
    let config = ok(&[(
        "a.toml",
        r#"
[check.disk]
command = "check_disk -p $path$"

[service.disk_root]
check = "disk"
vars.path = "/"

[[host]]
name = "h"
  [host.service.disk_root]
  vars.path = "/srv"
"#,
    )]);
    let s = config.service("h/disk_root").unwrap();
    assert_eq!(argv(s), ["check_disk", "-p", "/srv"]);
    assert_eq!(s.object.as_deref(), Some("disk_root"));
}

#[test]
fn for_each_over_a_nested_var() {
    let config = ok(&[(
        "a.toml",
        r#"
[check.http]
command = "check_http -H $host.vars.site.name$.$service.key$ -u $path$"

[service.web]
check = "http"
for_each = "site.tld"
vars.path = "/"

[hostgroup.web]
members = ["w*"]
services = ["web"]
vars.site.tld.eu = {}

[[host]]
name = "w1"
vars.site.name = "klarsocial"
vars.site.tld.de = { path = "/de" }
"#,
    )]);
    let eu = config.service("w1/web_eu").unwrap();
    assert_eq!(argv(eu), ["check_http", "-H", "klarsocial.eu", "-u", "/"]);
    let de = config.service("w1/web_de").unwrap();
    assert_eq!(argv(de), ["check_http", "-H", "klarsocial.de", "-u", "/de"]);
    assert_eq!(de.for_each, Some(("site.tld".into(), "de".into())));
    assert_eq!(de.sources["vars.path"].layer, Layer::ForEach);

    let (_, diags) = compile(&[(
        "a.toml",
        r#"
[service.ping]
command = "ping $service.key$"

[[host]]
name = "h"
services = ["ping"]
"#,
    )]);
    assert_eq!(
        messages(&diags)[0],
        "a.toml:2: error: host \"h\": service \"ping\": $service.key$ is not set"
    );
}

#[test]
fn default_host_check_is_the_builtin_ping() {
    let config = ok(&[(
        "a.toml",
        r#"
[check.check_ping]
command = "check_ping -H $host.address$ -w 50,5% -c 100,25%"

[service.check_ping]

[service.ping]
command = "my_ping $host.address$"

[hostgroup.quiet]
members = ["q*"]
host_check = false

[[host]]
name = "a"
address = "10.0.0.1"

[[host]]
name = "own"
address = "10.0.0.2"
services = ["check_ping"]
host_check = "check_ping"

[[host]]
name = "noaddress"

[[host]]
name = "off"
address = "10.0.0.3"
host_check = false

[[host]]
name = "q1"
address = "10.0.0.4"

[[host]]
name = "ownping"
address = "10.0.0.5"
services = ["ping"]
  [host.service.tuned]
  check = "ping"
  vars.packets = "2"
"#,
    )]);
    let host = |name: &str| config.hosts.iter().find(|h| h.name == name).unwrap();
    let services = |name: &str| -> Vec<&str> {
        config
            .services
            .iter()
            .filter(|s| config.host(s).name == name)
            .map(|s| s.name.as_str())
            .collect()
    };
    let ping = config.service("a/ping").unwrap();
    assert_eq!(host("a").host_check.as_deref(), Some("ping"));
    assert_eq!(ping.check.as_deref(), Some("ping"));
    assert_eq!(ping.title.as_deref(), Some("PING"));
    assert_eq!(
        ping.command.as_ref().unwrap().to_string(),
        "ping -H 10.0.0.1 -p 5 -w 3000,80% -c 5000,100% (built-in)"
    );
    assert!(
        matches!(&ping.assigned[0].reason, Reason::Listed { by, .. } if by == "default host check")
    );
    assert_eq!(host("own").host_check.as_deref(), Some("check_ping"));
    assert_eq!(services("own"), ["check_ping"]);
    for name in ["noaddress", "off", "q1"] {
        assert_eq!(host(name).host_check, None, "{name}");
        assert!(services(name).is_empty(), "{name}");
    }
    // A service "ping" of its own is the host check; the built-in by name.
    assert_eq!(host("ownping").host_check.as_deref(), Some("ping"));
    assert_eq!(
        argv(config.service("ownping/ping").unwrap()),
        ["my_ping", "10.0.0.5"]
    );
    assert_eq!(
        config
            .service("ownping/tuned")
            .unwrap()
            .command
            .as_ref()
            .unwrap()
            .to_string(),
        "ping -H 10.0.0.5 -p 2 -w 3000,80% -c 5000,100% (built-in)"
    );

    let (_, diags) = compile(&[(
        "a.toml",
        r#"
[check.x]
builtin = "smtp"

[check.y]
builtin = "ping"
command = "true"

[[host]]
name = "h"
host_check = true
"#,
    )]);
    assert_eq!(
        messages(&diags),
        [
            "a.toml:2: error: check \"x\": unknown builtin \"smtp\" (known: ping, tcp, http, dns, tls, proxmox-cluster, proxmox-node, proxmox-storage, proxmox-guest, proxmox-backup, proxmox-updates)",
            "a.toml:5: error: check \"y\": builtin \"ping\" runs without command or passive",
            "a.toml:9: error: host \"h\": host_check: a service name, or false for none",
        ]
    );
}

#[test]
fn titles_with_macros_and_for_each_keys() {
    let config = ok(&[(
        "a.toml",
        r#"
[check.cert]
title = "CERT"
command = "check_http -H $website$ -C 30"

[service.cert]
for_each = "sites"

[service.web]
title = "WEBSITE ($website$)"
check = "cert"
for_each = "sites"

[service.ping]
command = "ping $host.address$"

[[host]]
name = "w1"
services = ["cert", "web", "ping"]
vars.sites.eu = { website = "klarsocial.eu" }
"#,
    )]);
    let title = |id: &str| config.service(id).unwrap().title.as_deref();
    assert_eq!(title("w1/cert_eu"), Some("CERT (eu)"));
    assert_eq!(title("w1/web_eu"), Some("WEBSITE (klarsocial.eu)"));
    assert_eq!(title("w1/ping"), None);
}

#[test]
fn empty_for_each_entry_from_a_host_group() {
    let config = ok(&[(
        "a.toml",
        r#"
[service.ping]
command = "ping -p $path$"
for_each = "paths"
vars.path = "/"

[hostgroup.all]
members = ["*"]
services = ["ping"]
vars.paths.root = {}

[[host]]
name = "h"
"#,
    )]);
    assert_eq!(
        argv(config.service("h/ping_root").unwrap()),
        ["ping", "-p", "/"]
    );
}

#[test]
fn same_level_conflicts_are_errors() {
    let (_, diags) = compile(&[(
        "a.toml",
        r#"
[check.load]
command = "check_load"

[service.load]

[hostgroup.a]
host_check = "load"
vars.x = "1"
services = ["load"]

[hostgroup.b]
vars.x = "2"

[[host]]
name = "h"
groups = ["a", "b"]
"#,
    )]);
    let msgs = messages(&diags);
    assert!(
        msgs.iter().any(|m| m.contains(
            "host \"h\": host groups set vars.x differently: a (a.toml:7) and b (a.toml:12)"
        )),
        "{msgs:?}"
    );
}

#[test]
fn reports_problems_with_locations() {
    let (_, diags) = compile(&[
        (
            "a.toml",
            r#"
[check.load]
command = "check_load"

[hostgroup.g]
services = ["lod"]

[[host]]
name = "web01"
groups = ["nope"]

[[host]]
name = "web01"

[service.lonely]
"#,
        ),
        (
            "b.toml",
            r#"[check.y]
command = "check $host.adress$"
interval = "soon"

[[host]]
name = "h"
  [host.service.z]
  check = "missing"
  [host.service.w]
  command = "check_w -p $port$"
"#,
        ),
    ]);
    let msgs = messages(&diags);
    for expected in [
        "a.toml:5: error: hostgroup \"g\": unknown service \"lod\"",
        "a.toml:8: error: host \"web01\": unknown hostgroup \"nope\"",
        "a.toml:12: error: host \"web01\" is already defined at a.toml:8",
        "a.toml:15: error: service \"lonely\": no check \"lonely\"; set check, command or passive = true",
        "b.toml:1: error: check \"y\": invalid duration \"soon\"",
        "b.toml:7: error: host \"h\": service \"z\": unknown check \"missing\"",
        "b.toml:9: error: host \"h\": service \"w\": $port$ is not set",
    ] {
        assert!(
            msgs.iter().any(|m| m.starts_with(expected)),
            "missing {expected:?} in {msgs:#?}"
        );
    }
}

#[test]
fn old_format_gets_a_hint() {
    let hint = |text: &str| messages(&compile(&[("old.toml", text)]).1).join("\n");
    assert!(hint("[template.linux]\ninterval = \"1m\"\n").contains("templates are gone"));
    assert!(
        hint("[[check]]\nname = \"x\"\ncommand = \"true\"\n")
            .contains("[[check]] is the old format")
    );
    assert!(hint("[role.linux]\nservice.ping = {}\n").contains("roles are gone"));
    assert!(hint("[[host]]\nname = \"a\"\ntags = [\"web\"]\n").contains("tags are gone"));
}

#[test]
fn host_group_members_by_pattern() {
    let config = ok(&[(
        "a.toml",
        r#"
[check.ping]
command = "check_ping"

[service.ping]

[hostgroup.web]
members = ["web*"]
services = ["ping"]

[[host]]
name = "web01"

[[host]]
name = "db01"
"#,
    )]);
    assert!(config.hosts[0].in_group("web"));
    assert!(!config.hosts[1].in_group("web"));
    assert!(config.service("web01/ping").is_some());
    assert!(config.service("db01/ping").is_none());
}

#[test]
fn unused_and_unknown_names_warn() {
    let (_, diags) = compile(&[(
        "a.toml",
        r#"
[check.unused]
command = "x"

[service.idle]
command = "true"

[hostgroup.empty]

[contact.ops]
email = "ops@example.com"

[[notify]]
contacts = ["ops"]
hosts = { groups = ["no-such-group"] }

[[host]]
name = "h"
"#,
    )]);
    let msgs = messages(&diags);
    for expected in [
        "a.toml:2: warning: check \"unused\" is used by no service",
        "a.toml:5: warning: service \"idle\" is on no host",
        "a.toml:8: warning: hostgroup \"empty\" has no hosts",
        "a.toml:13: warning: selector: no hostgroup \"no-such-group\"",
    ] {
        assert!(
            msgs.contains(&expected.to_string()),
            "missing {expected:?} in {msgs:#?}"
        );
    }
}

#[test]
fn graphs_can_be_turned_off() {
    let config = ok(&[(
        "a.toml",
        r#"
[check.x]
command = "x"
graphs = false

[service.x]

[service.y]
check = "x"
graphs = true

[[host]]
name = "h"
services = ["x", "y"]
"#,
    )]);
    assert!(!config.service("h/x").unwrap().settings.graphs);
    assert!(config.service("h/y").unwrap().settings.graphs);
}

#[test]
fn inline_commands_and_shell() {
    let config = ok(&[(
        "a.toml",
        r#"
[[host]]
name = "h"
vars.label = "two words"
  [host.service.uptime]
  shell = true
  command = 'echo "up=$(cut -d" " -f1 /proc/uptime)s" $host.vars.label$'
"#,
    )]);
    let s = config.service("h/uptime").unwrap();
    assert_eq!(
        s.command,
        Some(CommandLine::Shell(
            "echo \"up=$(cut -d\" \" -f1 /proc/uptime)s\" 'two words'".into()
        ))
    );
    assert_eq!(s.check, None);
}

#[test]
fn downtime_groups() {
    let (config, diags) = compile(&[(
        "a.toml",
        r#"
[hostgroup.linux]
members = ["db*"]

[downtimegroup.patchday]
title = "Patch Day"
members = ["web01"]
hostgroups = ["linux"]
schedule = [{ days = ["sun"], from = "02:00", to = "04:00" }]

[downtimegroup.bad]
hostgroups = ["nope"]
schedule = [{ from = "02:00" }]

[[host]]
name = "web01"

[[host]]
name = "db01"

[[host]]
name = "mq01"
"#,
    )]);
    let msgs = messages(&diags);
    assert!(
        msgs.iter()
            .any(|m| m.contains("downtimegroup \"bad\": unknown hostgroup \"nope\"")),
        "{msgs:?}"
    );
    assert!(
        msgs.iter()
            .any(|m| m.contains("downtimegroup \"bad\": schedule 1: needs to")),
        "{msgs:?}"
    );
    let g = &config.downtimegroups[0];
    let hosts: Vec<_> = config
        .hosts
        .iter()
        .filter(|h| g.has_host(h))
        .map(|h| h.name.as_str())
        .collect();
    assert_eq!(hosts, ["web01", "db01"]);
    assert_eq!(g.schedule[0].to_string(), "sun 02:00-04:00");
}

#[test]
fn ssh_from_host_groups_and_hosts() {
    let config = ok(&[(
        "a.toml",
        r#"
[check.load]
command = "check_load -w 5 -c 10"
run_on = "host"

[service.load]

[service.local_load]
check = "load"
run_on = "local"

[hostgroup.linux]
members = ["web*"]
services = ["load", "local_load"]
ssh = { user = "monitor", secret = "monitor" }

[[host]]
name = "web1"

[[host]]
name = "web2"
ssh = { port = 2222, secret = "web2" }
"#,
    )]);
    let host = |name: &str| config.hosts.iter().find(|h| h.name == name).unwrap();
    assert_eq!(
        host("web1").ssh,
        Some(Ssh {
            user: "monitor".into(),
            port: 22,
            secret: "monitor".into(),
        })
    );
    assert_eq!(
        host("web2").ssh,
        Some(Ssh {
            user: "monitor".into(),
            port: 2222,
            secret: "web2".into(),
        })
    );
    assert_eq!(host("web2").sources["ssh.user"].layer, Layer::HostGroup);
    assert_eq!(host("web2").sources["ssh.secret"].layer, Layer::Host);

    let load = config.service("web1/load").unwrap();
    let local = config.service("web1/local_load").unwrap();
    assert_eq!(load.settings.run_on, RunOn::Host);
    assert_eq!(local.settings.run_on, RunOn::Local);
    // Same command, but on the host it measures something else.
    assert_ne!(load.check_hash, local.check_hash);
}

#[test]
fn ssh_errors() {
    let (_, diags) = compile(&[(
        "a.toml",
        r#"
[check.load]
command = "check_load"
run_on = "host"

[check.script]
command = "my_script"
shell = true
run_on = "host"
copy = true

[check.copied]
command = "my_script"
copy = true

[check.pinger]
builtin = "ping"
run_on = "host"

[service.load]
[service.script]
[service.copied]
[service.pinger]

[service.pushed]
passive = true
run_on = "host"

[hostgroup.half]
ssh = { user = "monitor" }

[hostgroup.typo]
ssh = { usr = "monitor" }

[[host]]
name = "nossh"
services = ["load"]

[[host]]
name = "half"
groups = ["half"]

[[host]]
name = "ok"
ssh = { user = "monitor", secret = "monitor" }
services = ["script", "copied", "pinger", "pushed"]
"#,
    )]);
    let errors = messages(&diags);
    let has = |text: &str| {
        assert!(
            errors.iter().any(|e| e.contains(text)),
            "{text:?} not in {errors:#?}"
        );
    };
    has("hostgroup \"typo\": ssh: unknown field `usr`");
    has("host \"half\": ssh needs user and secret");
    has("host \"nossh\": service \"load\": run_on = \"host\" needs ssh");
    has("service \"script\": copy needs a direct command, not shell = true");
    has("service \"copied\": copy needs run_on = \"host\"");
    has("service \"pinger\": built-in checks run locally");
    has("service \"pushed\": passive services have nothing to run on the host");
}

#[test]
fn http_headers_are_a_table_merged_across_layers() {
    let config = ok(&[(
        "a.toml",
        r#"
[check.api]
builtin = "http"
vars.headers = { Accept = "application/json", X-Api-Key = "$key$" }

[service.api]
check = "api"
vars.key = "k1"
vars.headers.X-Site = "$host.name$"

[[host]]
name = "h"
address = "10.0.0.5"
services = ["api"]
"#,
    )]);
    let s = &config.services[0];
    assert_eq!(
        s.command.as_ref().unwrap().to_string(),
        "http GET http://10.0.0.5/ header=\"Accept: application/json\" \
         header=\"X-Api-Key: ***\" header=\"X-Site: h\" (built-in)"
    );
    assert_eq!(s.vars["headers.X-Api-Key"], "k1");
    assert!(s.sources.contains_key("vars.headers.X-Site"));
}

#[test]
fn builtin_checks_are_services_too() {
    let (config, diags) = compile(&[(
        "a.toml",
        r#"
[servicegroup.web]
services = ["http", "tls"]

[hostgroup.dns-servers]
members = ["ns*"]
services = ["dns"]

[[host]]
name = "www"
address = "10.0.0.5"
services = ["tcp"]
servicegroups = ["web"]
  [host.service.tcp]
  vars.port = "22"

[[host]]
name = "ns1"
  [host.service.dns]
  vars.name = "example.com"
"#,
    )]);
    assert_eq!(messages(&diags), Vec::<String>::new());
    let ids: Vec<&str> = config.services.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(
        ids,
        ["www/http", "www/tcp", "www/tls", "www/ping", "ns1/dns"]
    );
    // Not service objects of their own: nothing to list or warn about.
    assert!(config.service_objects.is_empty());
    let s = config.service("www/tcp").unwrap();
    assert_eq!(
        s.command.as_ref().unwrap().to_string(),
        "tcp 10.0.0.5:22 (built-in)"
    );

    // Problems point at the host that lists it.
    let (_, diags) = compile(&[("a.toml", "[[host]]\nname = \"h\"\nservices = [\"tcp\"]\n")]);
    assert_eq!(
        messages(&diags),
        ["a.toml:1: error: host \"h\": service \"tcp\": vars.port is not set: the port"]
    );
}

#[test]
fn builtin_on_services() {
    let config = ok(&[(
        "a.toml",
        r#"
[service.ssh-port]
builtin = "tcp"
vars.port = "22"

[[host]]
name = "h"
address = "10.0.0.5"
services = ["ssh-port"]
  [host.service.web-port]
  builtin = "tcp"
  vars.port = "80"
"#,
    )]);
    let command = |id: &str| {
        let s = config.services.iter().find(|s| s.id == id).unwrap();
        s.command.as_ref().unwrap().to_string()
    };
    assert_eq!(command("h/ssh-port"), "tcp 10.0.0.5:22 (built-in)");
    assert_eq!(command("h/web-port"), "tcp 10.0.0.5:80 (built-in)");
}

#[test]
fn builtin_checks_take_their_settings_from_vars() {
    let config = ok(&[(
        "a.toml",
        r#"
[service.http]
vars = { url = "https://$host.name$/health", status = "200" }

[service.cert]
check = "tls"
vars = { name = "$host.name$", warn = "$days$", days = "21" }

[service.ssh]
check = "tcp"
vars = { expect = "SSH-", port = 22 }

[service.ssh_alt]
check = "tcp"
vars.port = "$host.vars.port$"

[[host]]
name = "www.example.com"
address = "10.0.0.5"
vars.port = "2222"
services = ["http", "cert", "ssh", "ssh_alt"]
"#,
    )]);
    let command = |id: &str| {
        let s = config.services.iter().find(|s| s.id == id).unwrap();
        s.command.as_ref().unwrap().to_string()
    };
    assert_eq!(
        command("www.example.com/http"),
        "http GET https://www.example.com/health status=200 (built-in)"
    );
    assert_eq!(
        command("www.example.com/cert"),
        "tls 10.0.0.5:443 name=www.example.com warn=21d crit=14d (built-in)"
    );
    // Host vars don't reach built-in checks, except through macros.
    assert_eq!(
        command("www.example.com/ssh"),
        "tcp 10.0.0.5:22 expect=\"SSH-\" (built-in)"
    );
    assert_eq!(
        command("www.example.com/ssh_alt"),
        "tcp 10.0.0.5:2222 (built-in)"
    );

    let (_, diags) = compile(&[(
        "a.toml",
        r#"
[service.dns]
vars.name = "$nope$"

[service.tcp]

[[host]]
name = "h"
services = ["dns", "tcp"]
"#,
    )]);
    assert_eq!(
        messages(&diags),
        [
            "a.toml:2: error: host \"h\": service \"dns\": vars.name: $nope$ is not set",
            "a.toml:2: warning: service \"dns\" is on no host",
            "a.toml:5: error: host \"h\": service \"tcp\": vars.port is not set: the port",
            "a.toml:5: warning: service \"tcp\" is on no host",
        ]
    );
}

#[test]
fn time_periods_limit_notify_rules() {
    let base = r#"
[contact.day]
[contact.night]

[[host]]
name = "a"
  [host.service.disk]
  command = "true"
"#;
    let config = ok(&[(
        "a.toml",
        &format!(
            r#"{base}
[timeperiod.workhours]
title = "Business hours"
schedule = [{{ days = ["mon", "tue", "wed", "thu", "fri"], from = "08:00", to = "18:00" }}]

[timeperiod.offhours]
schedule = [
  {{ days = ["mon", "tue", "wed", "thu", "fri"], from = "00:00", to = "08:00" }},
  {{ days = ["mon", "tue", "wed", "thu", "fri"], from = "18:00", to = "00:00" }},
  {{ days = ["sat", "sun"], from = "00:00", to = "00:00" }},
]

[[notify]]
contacts = ["day"]
period = "workhours"

[[notify]]
contacts = ["night"]
period = "offhours"
"#
        ),
    )]);
    assert_eq!(config.timeperiods.len(), 2);
    assert_eq!(config.notify[0].period.as_deref(), Some("workhours"));
    let s = config.service("a/disk").unwrap();
    let tz = crate::TimeZone::UTC;
    // Monday 2026-11-09, and Saturday.
    let at = |days: u64, h: u64| crate::At {
        now: std::time::UNIX_EPOCH + Duration::from_secs(1_794_182_400 + days * 86400 + h * 3600),
        tz: &tz,
    };
    let to = |alert, at| -> Vec<String> {
        config
            .recipients(s, false, alert, "critical", at)
            .iter()
            .map(|c| c.name.clone())
            .collect()
    };
    assert_eq!(to(Alert::Problem, Some(at(0, 10))), ["day"]);
    assert_eq!(to(Alert::Problem, Some(at(0, 20))), ["night"]);
    assert_eq!(to(Alert::Problem, Some(at(5, 10))), ["night"]);
    // Monday early morning, after the weekend.
    assert_eq!(to(Alert::Problem, Some(at(0, 3))), ["night"]);
    // Without a time: everyone who could be notified.
    assert_eq!(to(Alert::Problem, None), ["day", "night"]);
    // Recoveries only through followup_recoveries (who got the problem).
    assert!(to(Alert::Recovery, Some(at(0, 10))).is_empty());
    assert_eq!(config.followup_recoveries(s, false, "critical").len(), 2);
    let f = config.followups(s, false, "critical");
    assert_eq!(f.len(), 2);
    assert_eq!(f[0].period.map(|p| p.name.as_str()), Some("workhours"));
    assert_eq!(f[0].after, None);

    let (_, diags) = compile(&[(
        "a.toml",
        &format!(
            r#"{base}
[timeperiod.empty]

[timeperiod.bad]
schedule = [{{ days = ["someday"], from = "08:00", to = "18:00" }}]

[timeperiod.unused]
schedule = [{{ from = "08:00", to = "18:00" }}]

[[notify]]
contacts = ["day"]
period = "nope"
"#
        ),
    )]);
    let text = messages(&diags).join("\n");
    assert!(
        text.contains("timeperiod \"empty\": needs a schedule"),
        "{text}"
    );
    assert!(text.contains("timeperiod \"bad\": schedule 1:"), "{text}");
    assert!(text.contains("unknown timeperiod \"nope\""), "{text}");
    assert!(
        text.contains("timeperiod \"unused\" is used by no notify rule"),
        "{text}"
    );
}

#[test]
fn dependencies_match_like_notify_rules() {
    let config = ok(&[(
        "a.toml",
        r#"
[check.ping]
command = "check_ping -H $host.address$"

[check.http]
command = "check_http -H $host.address$"

[service.ping]
[service.http]

[[host]]
name = "gw"
host_check = "ping"
services = ["ping"]

[[host]]
name = "web01"
services = ["ping", "http"]

[[host]]
name = "db01"
host_check = "ping"
services = ["ping", "http"]

[[dependency]]
name = "behind-gw"
on = "gw"
hosts = ["web01", "db01"]

[[dependency]]
on = "db01/http"
services = { name = ["http"] }
"#,
    )]);
    assert_eq!(config.dependencies.len(), 2);
    let behind = &config.dependencies[0];
    assert_eq!(behind.states, ["down"]);
    assert_eq!(behind.master(), ("gw", None));
    let on_http = &config.dependencies[1];
    assert_eq!(on_http.states, ["critical", "unknown"]);
    assert_eq!(on_http.label(), "dependency at a.toml:30");

    let dep_names = |id: &str, host_alert: bool| -> Vec<String> {
        let s = config.service(id).unwrap();
        config
            .matching_dependencies(s, host_alert)
            .into_iter()
            .map(|(d, _)| d.label())
            .collect()
    };
    // Services and host alerts of hosts behind the gateway wait for it;
    // the service dependency only takes service alerts.
    assert_eq!(
        dep_names("web01/http", false),
        ["behind-gw", "dependency at a.toml:30"]
    );
    assert_eq!(dep_names("db01/ping", true), ["behind-gw"]);
    // The master's own alert is never held back: not db01/http for the
    // second dependency, and not the gateway's own host check.
    assert_eq!(dep_names("db01/http", false), ["behind-gw"]);
    assert_eq!(dep_names("gw/ping", true), Vec::<String>::new());

    // The explanation mirrors notify rules.
    let s = config.service("web01/http").unwrap();
    let (_, why) = &config.matching_dependencies(s, false)[0];
    assert_eq!(why, "host: listed");
}

#[test]
fn dependency_errors() {
    let (_, diags) = compile(&[(
        "a.toml",
        r#"
[check.ping]
command = "check_ping -H $host.address$"

[service.ping]

[[host]]
name = "gw"
services = ["ping"]

[[dependency]]
name = "bad"
on = "nowhere"

[[dependency]]
name = "bad-service"
on = "gw/http"

[[dependency]]
name = "bad-states"
on = "gw"
states = ["critical"]

[[dependency]]
name = "never"
on = "gw"

[[dependency]]
name = "never"
on = "gw"
"#,
    )]);
    let text = messages(&diags).join("\n");
    assert!(
        text.contains("dependency \"bad\": on: unknown host \"nowhere\""),
        "{text}"
    );
    assert!(
        text.contains("dependency \"bad-service\": on: unknown service \"gw/http\""),
        "{text}"
    );
    assert!(
        text.contains("dependency \"bad-states\": unknown state \"critical\" (expected down)"),
        "{text}"
    );
    assert!(
        text.contains("name is already used by another dependency"),
        "{text}"
    );
    // gw has no host check: it is always UP, the dependency never applies.
    assert!(
        text.contains("dependency \"never\": host \"gw\" has no host check"),
        "{text}"
    );
}

#[test]
fn jira_channel_defaults_and_errors() {
    let config = ok(&[(
        "a.toml",
        r#"
[contact.helpdesk]
jira = { url = "https://acme.atlassian.net/", project = "OPS", email = "bot@acme.com", token = "t0ken" }

[contact.dc]
jira = { url = "https://jira.example.com", project = "OPS", token = "pat", issue_type = "Incident", close = "Done" }

[[notify]]
contacts = ["helpdesk", "dc"]
"#,
    )]);
    let by_name = |n: &str| {
        config
            .contacts
            .iter()
            .find(|c| c.name == n)
            .unwrap()
            .channels
            .jira
            .as_ref()
            .unwrap()
    };
    let jira = by_name("helpdesk");
    // Cloud is detected from the host, the URL loses its trailing slash,
    // and the issue type falls back to Task.
    assert!(jira.cloud);
    assert_eq!(jira.url, "https://acme.atlassian.net");
    assert_eq!(jira.issue_type, "Task");
    assert_eq!(jira.close, None);
    let dc = by_name("dc");
    assert!(!dc.cloud);
    assert_eq!(dc.issue_type, "Incident");
    assert_eq!(dc.close.as_deref(), Some("Done"));

    let (_, diags) = compile(&[(
        "a.toml",
        r#"
[contact.no-email]
jira = { url = "https://acme.atlassian.net", project = "OPS", token = "t" }

[contact.pat-with-email]
jira = { url = "https://jira.example.com", project = "OPS", email = "x@y.de", token = "t" }

[contact.bad]
jira = { url = "ftp://jira", project = " ", token = "" }
"#,
    )]);
    let text = messages(&diags).join("\n");
    assert!(
        text.contains("contact \"no-email\": jira: Jira Cloud needs email"),
        "{text}"
    );
    assert!(
        text.contains(
            "contact \"pat-with-email\": jira: Data Center/Server uses a personal access token"
        ),
        "{text}"
    );
    assert!(
        text.contains("contact \"bad\": jira.url must be an http(s) URL"),
        "{text}"
    );
    assert!(
        text.contains("contact \"bad\": jira.project is empty"),
        "{text}"
    );
    assert!(
        text.contains("contact \"bad\": jira.token is empty"),
        "{text}"
    );
}

#[test]
fn node_local_is_this_instance() {
    let (config, diags) = compile(&[(
        "conf.d/hosts.toml",
        r#"
[service.show]
command = "echo $host.node$"

[contact.ops]
email = "ops@example.com"

[[host]]
name = "branch-router"
node = "branch-office"
services = ["show"]

[[host]]
name = "hq-router"
node = "local"
services = ["show"]

[[host]]
name = "hq-nas"
services = ["show"]

[[notify]]
contacts = ["ops"]
hosts = { node = "local" }
"#,
    )]);
    // `local` is always known: no warning about an unknown node.
    assert_eq!(messages(&diags), Vec::<String>::new());
    let host = |name: &str| config.hosts.iter().find(|h| h.name == name).unwrap();
    assert_eq!(host("hq-router").node, None);
    assert_eq!(host("hq-router").node_name(), "local");
    assert_eq!(host("branch-router").node_name(), "branch-office");
    let show = |h: &str| argv(config.service(&format!("{h}/show")).unwrap()).join(" ");
    assert_eq!(show("hq-nas"), "echo local");
    assert_eq!(show("branch-router"), "echo branch-office");
    let local = HostSelector::parse(&toml::Value::Table(
        toml::from_str(r#"node = "local""#).unwrap(),
    ))
    .unwrap();
    let matched: Vec<&str> = config
        .hosts
        .iter()
        .filter(|h| local.matches(h).is_some())
        .map(|h| h.name.as_str())
        .collect();
    assert_eq!(matched, ["hq-router", "hq-nas"]);
}

#[test]
fn proxmox_hosts() {
    let config = ok(&[(
        "a.toml",
        r#"
# The login for every Proxmox VE node.
[hostgroup.pve]
members = ["pve*"]
proxmox = { secret = "pve", fingerprint = "18:B4:0D:EC:C4:69:B9:5C:8B:30:C2:82:ED:4A:0C:B9:49:01:0A:3B:04:0F:55:86:68:A8:85:40:6F:22:F3:A2" }
services = ["proxmox-node"]

[[host]]
name = "pve1"
address = "192.0.2.10"
proxmox = { discover = true }
services = ["proxmox-cluster", "proxmox-storage"]

[[host]]
name = "jellyfin"
address = "192.0.2.71"
proxmox = { host = "pve1", vmid = 100 }
services = ["proxmox-guest"]
"#,
    )]);
    let command = |id: &str| {
        let s = config.service(id).unwrap();
        s.command.as_ref().unwrap().to_string()
    };
    assert_eq!(
        command("pve1/proxmox-node"),
        "proxmox-node https://192.0.2.10:8006 secret=pve fingerprint=18:B4:0D:EC:… (built-in)"
    );
    assert_eq!(
        command("pve1/proxmox-storage"),
        "proxmox-storage https://192.0.2.10:8006 warn=80% crit=90% secret=pve fingerprint=18:B4:0D:EC:… (built-in)"
    );
    // A guest asks its Proxmox VE host, with the login from there.
    assert_eq!(
        command("jellyfin/proxmox-guest"),
        "proxmox-guest https://192.0.2.10:8006 vmid=100 secret=pve fingerprint=18:B4:0D:EC:… (built-in)"
    );
    let names: Vec<&str> = config.discoveries.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["pve1"]);
    assert_eq!(
        config.discoveries[0].connection.url,
        "https://192.0.2.10:8006"
    );
    assert_eq!(config.discoveries[0].file(), "discovered/pve1.toml");

    let (_, diags) = compile(&[(
        "a.toml",
        r#"
[[host]]
name = "a"
proxmox = { host = "nope", vmid = 100 }

[[host]]
name = "b"
proxmox = { vmid = 100 }

[[host]]
name = "c"
proxmox = { host = "a", vmid = 100, secret = "x" }

[[host]]
name = "d"
proxmox = { discover = true }

[[host]]
name = "e"
proxmox = { host = "a", vmid = 101 }

[[host]]
name = "f"
services = ["proxmox-node"]

[[host]]
name = "g"
proxmox = { secret = "s", fingerprint = "12:34" }
"#,
    )]);
    assert_eq!(
        messages(&diags),
        [
            "a.toml:2: error: host \"a\": proxmox.host \"nope\": no such host",
            "a.toml:6: error: host \"b\": proxmox: a VM or container needs host and vmid",
            "a.toml:10: error: host \"c\": proxmox: a VM or container (host, vmid) takes the login from its Proxmox VE host; secret, port, fingerprint, verify and discover go there",
            "a.toml:14: error: host \"d\": proxmox: needs secret (an API token), or host and vmid for a VM or container",
            "a.toml:18: error: host \"e\": proxmox.host \"a\" has no proxmox = { secret = ... }",
            "a.toml:22: error: host \"f\": service \"proxmox-node\": proxmox-node needs proxmox = { secret = \"...\" } on the host or a host group (for a VM or container: proxmox = { host = \"<Proxmox VE host>\", vmid = 100 })",
            "a.toml:26: error: host \"g\": proxmox: fingerprint \"12:34\": expected a SHA-256 fingerprint, e.g. 18:B4:0D:… (64 hex digits)",
        ]
    );
}

#[test]
fn host_check_brings_its_service() {
    // As in the web UI: check_ping is assigned through a service group the
    // host isn't in, and named as its host check.
    let config = ok(&[(
        "a.toml",
        r#"
[check.check_ping]
command = "check_ping -H $host.address$ -w 100,20% -c 500,60%"

[service.check_ping]
groups = ["linux"]

[servicegroup.linux]

[hostgroup.pve]
members = ["pve*"]
host_check = "check_ping"

[[host]]
name = "PROXMOX"
address = "10.0.0.1"
host_check = "check_ping"

[[host]]
name = "pve2"
address = "10.0.0.2"
"#,
    )]);
    for name in ["PROXMOX", "pve2"] {
        let id = format!("{name}/check_ping");
        let s = config.service(&id).unwrap_or_else(|| panic!("no {id}"));
        assert!(matches!(&s.assigned[0].reason, Reason::Listed { by, .. } if by == "host_check"));
        let host = config.hosts.iter().find(|h| h.name == name).unwrap();
        assert_eq!(host.host_check.as_deref(), Some("check_ping"));
    }
    // No ping besides it.
    assert!(config.service("PROXMOX/ping").is_none());
}

#[test]
fn hints_for_names() {
    let (_, diags) = compile(&[(
        "a.toml",
        r#"
[[host]]
name = "PROXMOX"
address = "10.0.0.1"
services = ["tcp*", "tls"]
host_check = "check_pingg"
"#,
    )]);
    assert_eq!(
        messages(&diags),
        [
            "a.toml:2: error: host \"PROXMOX\": unknown service \"tcp*\": services = [...] takes names, not patterns (matching: tcp)",
        ]
    );
    let (_, diags) = compile(&[(
        "a.toml",
        r#"
[[host]]
name = "PROXMOX"
address = "10.0.0.1"
services = ["tls"]
host_check = "check_pingg"
"#,
    )]);
    assert_eq!(
        messages(&diags),
        [
            "a.toml:2: error: host \"PROXMOX\": host_check \"check_pingg\" is not a service on it; it names one of its services: tls; without host_check, a host with an address uses ping",
        ]
    );
}

#[test]
fn hints_for_host_check() {
    // A host check that is turned off, and one with errors of its own:
    // each says what is wrong, reported once.
    let (_, diags) = compile(&[(
        "a.toml",
        r#"
[[host]]
name = "b"
host_check = "ping"
service.ping.enabled = false

[[host]]
name = "c"
host_check = "tcp"
"#,
    )]);
    assert_eq!(
        messages(&diags),
        [
            "a.toml:2: error: host \"b\": host_check \"ping\" is turned off on it (enabled = false)",
            "a.toml:7: error: host \"c\": service \"tcp\": vars.port is not set: the port",
        ]
    );
}

#[test]
fn graph_falls_back_to_the_host_check() {
    // The host group's graph fits "a", which has `load`; "b" doesn't have
    // it and "c" has its graphs off: both show their host check's graph.
    let (config, diags) = compile(&[(
        "a.toml",
        r#"
[service.load]
command = "true"

[hostgroup.g]
members = ["*"]
graph = "load"

[[host]]
name = "a"
address = "10.0.0.1"
services = ["load"]

[[host]]
name = "b"
address = "10.0.0.2"

[[host]]
name = "c"
address = "10.0.0.3"
services = ["load"]
service.load.graphs = false
"#,
    )]);
    assert_eq!(messages(&diags), Vec::<String>::new());
    let graph = |name: &str| {
        let host = config.hosts.iter().find(|h| h.name == name).unwrap();
        (host.graph_check(), host.sources.contains_key("graph"))
    };
    assert_eq!(graph("a"), (Some("load"), true));
    assert_eq!(graph("b"), (Some("ping"), false));
    assert_eq!(graph("c"), (Some("ping"), false));
}

#[test]
fn checks_can_be_listed_as_services() {
    // No [service.x] for any of them: a check is a service as it is.
    let (config, diags) = compile(&[(
        "a.toml",
        r#"
[check.load]
command = "check_load -w $warn$"
vars.warn = "5"

[check.swap]
command = "check_swap"

[check.alive]
command = "check_alive $host.address$"

[servicegroup.linux]
services = ["swap"]

[hostgroup.g]
members = ["web01"]
servicegroups = ["linux"]

[[host]]
name = "web01"
address = "10.0.0.1"
services = ["load"]
host_check = "alive"
  [host.service.load]
  vars.warn = "8"
"#,
    )]);
    assert_eq!(messages(&diags), Vec::<String>::new());
    let load = config.service("web01/load").unwrap();
    assert_eq!(load.check.as_deref(), Some("load"));
    assert_eq!(load.command_text.as_deref(), Some("check_load -w $warn$"));
    assert_eq!(load.vars["warn"], "8");
    assert!(config.service("web01/swap").is_some());
    assert!(config.service("web01/alive").is_some());
    // They aren't service objects of the config.
    assert!(config.service_objects.is_empty());
}

#[test]
fn selector_takes_builtin_checks() {
    let (config, diags) = compile(&[(
        "a.toml",
        r#"
[contact.ops]
email = "ops@example.com"

[[notify]]
contacts = ["ops"]
services = { checks = ["http"] }

[[host]]
name = "h"
address = "10.0.0.1"
services = ["http"]
"#,
    )]);
    assert_eq!(messages(&diags), Vec::<String>::new());
    assert_eq!(
        config.service("h/http").and_then(|s| s.check.as_deref()),
        Some("http")
    );
}
