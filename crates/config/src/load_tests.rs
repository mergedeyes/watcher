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
[check.ping]
command = "check_ping"

[hostgroup.g]
services = ["ping"]

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
        "a.toml:5: error: hostgroup \"g\": unknown service \"ping\" (there is a check \"ping\": add [service.ping])",
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
