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

const ROLES: &str = r#"
[role.linux]
host_check = "ping"
interval = "1m"
groups = ["linux-servers"]
service.ping = {}
service.load = { check = "load" }
service.disk = { check = "disk", for_each = "disks", groups = ["storage"] }

[role.web]
hosts = { tags = ["web"] }
service.http = { check = "http" }
"#;

const HOSTS: &str = r#"
[[host]]
name = "web01"
address = "10.0.0.5"
roles = ["linux"]
tags = ["web", "prod"]
vars.disks = { root = { path = "/" } }

[[host]]
name = "db01"
roles = ["linux"]
tags = ["prod"]
groups = ["dc1"]
vars.disks.root = { path = "/" }
vars.disks.pgdata = { path = "/var/lib/postgresql", warn = "10%", crit = "5%" }

  [host.service.backup]
  passive = true
  freshness = "26h"

  [host.service.load]
  enabled = false
"#;

const GROUPS: &str = r#"
[hostgroup.dc1]
title = "Datacenter 1"

[hostgroup.linux-servers]

[servicegroup.storage]

[servicegroup.webshop]
title = "Webshop"
members = ["web01/http", "db01/disk_pgdata"]

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
hosts = { tags = ["prod"] }

[[notify]]
name = "dba-critical"
contact_groups = ["dba"]
services = { groups = ["webshop"] }
states = ["critical"]
"#;

fn example() -> Config {
    ok(&[
        ("checks.toml", CHECKS),
        ("roles.toml", ROLES),
        ("hosts.toml", HOSTS),
        ("groups.toml", GROUPS),
    ])
}

#[test]
fn compiles_the_design_example() {
    let (_, diags) = compile(&[
        ("checks.toml", CHECKS),
        ("roles.toml", ROLES),
        ("hosts.toml", HOSTS),
        ("groups.toml", GROUPS),
    ]);
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
            "db01/disk_pgdata",
            "db01/disk_root",
            "db01/ping",
            "db01/backup",
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
    assert_eq!(pgdata.role.as_deref(), Some("linux"));
    assert_eq!(
        pgdata.for_each,
        Some(("disks".to_string(), "pgdata".to_string()))
    );
    // Where values came from.
    assert_eq!(pgdata.sources["vars.warn"].layer, Layer::ForEach);
    assert_eq!(pgdata.sources["interval"].layer, Layer::Role);
    assert_eq!(pgdata.sources["command"].layer, Layer::Check);
    assert_eq!(pgdata.origin.line, 8);
    let groups: Vec<_> = pgdata.groups.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(groups, ["storage", "webshop"]);

    let root = config.service("web01/disk_root").unwrap();
    assert_eq!(
        argv(root),
        ["check_disk", "-w", "20%", "-c", "10%", "-p", "/"]
    );

    let http = config.service("web01/http").unwrap();
    assert_eq!(argv(http), ["check_http", "-H", "10.0.0.5", "-u", "/"]);
    assert!(http.settings.thresholds["time"].warn.is_some());

    let backup = config.service("db01/backup").unwrap();
    assert!(backup.settings.passive);
    assert_eq!(backup.settings.max_attempts, 1);
    assert_eq!(backup.origin.line, 17);

    // Roles: listed and by selector.
    let web = &config.hosts[0];
    let roles: Vec<_> = web.roles.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(roles, ["linux", "web"]);
    assert!(
        matches!(&web.roles[1].reason, Reason::Selector { matched, .. } if matched == "tag web")
    );
    assert_eq!(web.host_check.as_deref(), Some("ping"));
    assert_eq!(web.sources["host_check"].name, "linux");
    let db = &config.hosts[1];
    let groups: Vec<_> = db.groups.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(groups, ["dc1", "linux-servers"]);
    assert_eq!(config.host_check(db).unwrap().id, "db01/ping");

    // Contacts: a group lists alice, bob lists the group.
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
            .recipients(s, host_alert, alert, state)
            .iter()
            .map(|c| c.name.clone())
            .collect()
    };
    assert_eq!(
        names("db01/disk_pgdata", Alert::Problem, "critical"),
        ["oncall", "alice", "bob"]
    );
    assert_eq!(
        names("db01/disk_pgdata", Alert::Problem, "warning"),
        ["oncall"]
    );
    // A recovery from WARNING doesn't reach the critical-only rule.
    assert_eq!(
        names("db01/disk_pgdata", Alert::Recovery, "warning"),
        ["oncall"]
    );
    assert_eq!(
        names("web01/http", Alert::Problem, "critical"),
        ["oncall", "alice", "bob"]
    );
    // The host check alerts as the host: rules with services don't apply.
    assert_eq!(names("db01/ping", Alert::Problem, "down"), ["oncall"]);
    assert_eq!(state_name(State::Critical, true), "down");
}

#[test]
fn precedence_host_over_role_over_check() {
    let config = ok(&[(
        "a.toml",
        r#"
[check.http]
command = "check_http -u $url$ -t $timeout_s$ -p $port$"
vars = { url = "/", timeout_s = "10", port = "80" }
interval = "5m"
timeout = "20s"

[role.web]
interval = "2m"
vars.port = "8080"
service.http = { vars.url = "/status", timeout = "30s" }

[[host]]
name = "a"
roles = ["web"]

[[host]]
name = "b"
roles = ["web"]
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
    assert_eq!(a.sources["vars.port"].layer, Layer::Role);
    let b = config.service("b/http").unwrap();
    // Host vars beat the role's service entry; the host's service beats all.
    assert_eq!(
        argv(b),
        ["check_http", "-u", "/health", "-t", "5", "-p", "8080"]
    );
    assert_eq!(b.settings.interval, Duration::from_secs(10));
    assert_eq!(b.sources["interval"].layer, Layer::HostService);
}

#[test]
fn includes_merge_and_the_including_role_wins() {
    let config = ok(&[(
        "a.toml",
        r#"
[check.load]
command = "check_load -w $w$"
vars.w = "5"

[role.linux]
host_check = "load"
service.load = {}

[role.db]
include = ["linux"]
service.load = { vars.w = "9" }

[[host]]
name = "a"
roles = ["db", "linux"]
"#,
    )]);
    let s = config.service("a/load").unwrap();
    assert_eq!(argv(s), ["check_load", "-w", "9"]);
    assert_eq!(s.role.as_deref(), Some("db"));
    let host = &config.hosts[0];
    assert_eq!(host.host_check.as_deref(), Some("load"));
    let roles: Vec<_> = host.roles.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(roles, ["db", "linux"]);
}

#[test]
fn same_level_conflicts_are_errors() {
    let (_, diags) = compile(&[(
        "a.toml",
        r#"
[check.load]
command = "check_load"

[role.a]
host_check = "load"
vars.x = "1"
service.load = { interval = "1m" }

[role.b]
vars.x = "2"
service.load = { interval = "2m" }

[[host]]
name = "h"
roles = ["a", "b"]
"#,
    )]);
    let msgs = messages(&diags);
    assert!(
        msgs.iter().any(|m| m
            .contains("host \"h\": roles set vars.x differently: a (a.toml:5) and b (a.toml:10)")),
        "{msgs:?}"
    );
}

#[test]
fn services_from_two_roles_must_agree() {
    let (_, diags) = compile(&[(
        "a.toml",
        r#"
[check.load]
command = "check_load"

[role.a]
service.load = { interval = "1m" }

[role.b]
service.load = { interval = "2m" }

[role.c]
service.load = { interval = "2m" }

[[host]]
name = "h"
roles = ["a", "b"]

[[host]]
name = "same"
roles = ["b", "c"]
"#,
    )]);
    let msgs = messages(&diags);
    assert!(
        msgs.iter()
            .any(|m| m.contains("service \"load\" comes from roles \"a\" and \"b\"")),
        "{msgs:?}"
    );
    assert!(!msgs.iter().any(|m| m.contains("\"same\"")), "{msgs:?}");
}

#[test]
fn reports_problems_with_locations() {
    let (_, diags) = compile(&[
        (
            "a.toml",
            r#"
[role.a]
include = ["b"]

[role.b]
include = ["a"]

[role.sel]
hosts = { roles = ["x"] }

[[host]]
name = "web01"
roles = ["nope"]

[[host]]
name = "web01"
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
        "a.toml:2: error: include cycle: a -> b -> a",
        "a.toml:8: error: role \"sel\": hosts: can't select by roles or groups",
        "a.toml:11: error: host \"web01\": unknown role \"nope\"",
        "a.toml:15: error: host \"web01\" is already defined at a.toml:11",
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
    let (_, diags) = compile(&[(
        "old.toml",
        r#"
[template.linux]
interval = "1m"
"#,
    )]);
    assert!(
        messages(&diags)[0].contains("templates are gone"),
        "{diags:?}"
    );
    let (_, diags) = compile(&[(
        "old.toml",
        r#"
[[check]]
name = "x"
command = "true"
"#,
    )]);
    assert!(
        messages(&diags)[0].contains("[[check]] is the old format"),
        "{diags:?}"
    );
}

#[test]
fn group_selectors() {
    let config = ok(&[(
        "a.toml",
        r#"
[check.ping]
command = "check_ping"

[role.base]
hosts = {}
service.ping = {}

[hostgroup.dc1]
members = { node = "dc1" }

[servicegroup.pings]
members = { name = "ping", hosts = { groups = ["dc1"] } }

[[host]]
name = "a"
node = "dc1"

[[host]]
name = "b"
"#,
    )]);
    assert!(config.hosts[0].in_group("dc1"));
    assert!(!config.hosts[1].in_group("dc1"));
    assert!(config.service("a/ping").unwrap().in_group("pings"));
    assert!(!config.service("b/ping").unwrap().in_group("pings"));
}

#[test]
fn unused_and_unknown_names_warn() {
    let (_, diags) = compile(&[(
        "a.toml",
        r#"
[check.unused]
command = "x"

[role.lonely]
hosts = { tags = ["no-such-tag"] }

[[host]]
name = "h"
"#,
    )]);
    let msgs = messages(&diags);
    for expected in [
        "a.toml:2: warning: check \"unused\" is used by no service",
        "a.toml:5: warning: role \"lonely\" is assigned to no host",
        "a.toml:5: warning: selector: no tag \"no-such-tag\"",
    ] {
        assert!(
            msgs.contains(&expected.to_string()),
            "missing {expected:?} in {msgs:#?}"
        );
    }
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
