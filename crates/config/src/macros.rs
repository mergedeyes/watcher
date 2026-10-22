//! Command macros: `$host.address$`, `$host.vars.port$`, `$check.name$`, ...
//!
//! Only `$host.…$` and `$check.…$` (plus the Nagios forms `$HOSTNAME$` and
//! `$HOSTADDRESS$`) are macros; any other `$` is left alone, so shell
//! variables like `$HOME` or `$1` keep working.
//!
//! Values are substituted after the command is split into arguments, so a
//! value with spaces stays one argument. In shell mode values are quoted for
//! `/bin/sh` instead; don't wrap macros in quotes there.

use crate::{Check, Host};
use watcher_exec::CommandLine;

/// Expands macros in a check's command for one host.
pub fn expand(check: &Check, host: &Host) -> Result<CommandLine, String> {
    let lookup = |name: &str| -> Result<String, String> {
        let value = match name.split('.').collect::<Vec<_>>().as_slice() {
            ["host", "name"] | ["HOSTNAME"] => Some(host.name.clone()),
            ["host", "address"] | ["HOSTADDRESS"] => Some(host.address.clone()),
            ["host", "node"] => host.node.clone(),
            ["host", "vars", var] => host.vars.get(*var).cloned(),
            ["check", "name"] => Some(check.name.clone()),
            ["check", "vars", var] => check.vars.get(*var).cloned(),
            _ => return Err(format!("unknown macro ${name}$")),
        };
        value.ok_or_else(|| format!("${name}$ is not set for host {:?}", host.name))
    };

    if check.shell {
        let script = substitute(&check.command, |name| {
            lookup(name).map(|v| shell_words::quote(&v).into_owned())
        })?;
        Ok(CommandLine::Shell(script))
    } else {
        let argv =
            shell_words::split(&check.command).map_err(|e| format!("cannot split command: {e}"))?;
        let argv = argv
            .iter()
            .map(|arg| substitute(arg, lookup))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(CommandLine::Argv(argv))
    }
}

/// Whether `name` (without the `$`s) is a macro that [`expand`] knows.
pub fn is_known(name: &str) -> bool {
    matches!(
        name.split('.').collect::<Vec<_>>().as_slice(),
        ["host", "name" | "address" | "node"]
            | ["host" | "check", "vars", _]
            | ["check", "name"]
            | ["HOSTNAME" | "HOSTADDRESS"]
    )
}

/// Lists the macros used in `text`, for validation without a host.
pub fn names(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some((_, name, after)) = next_macro(rest) {
        out.push(name);
        rest = after;
    }
    out
}

/// Replaces every macro in `text` with `lookup(name)`.
fn substitute(
    text: &str,
    mut lookup: impl FnMut(&str) -> Result<String, String>,
) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some((before_len, name, after)) = next_macro(rest) {
        out.push_str(&rest[..before_len]);
        out.push_str(&lookup(name)?);
        rest = after;
    }
    out.push_str(rest);
    Ok(out)
}

/// Finds the next `$name$` whose name is a macro. Returns the length of the
/// text before it, the name, and the text after it.
fn next_macro(text: &str) -> Option<(usize, &str, &str)> {
    let mut from = 0;
    while let Some(start) = text[from..].find('$').map(|i| from + i) {
        let body = &text[start + 1..];
        let len = body
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-'))
            .unwrap_or(body.len());
        let name = &body[..len];
        let is_macro = name.starts_with("host.")
            || name.starts_with("check.")
            || name == "HOSTNAME"
            || name == "HOSTADDRESS";
        if is_macro && body[len..].starts_with('$') {
            return Some((start, name, &body[len + 1..]));
        }
        from = start + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn host() -> Host {
        Host {
            name: "web01".into(),
            address: "10.0.0.5".into(),
            node: None,
            tags: vec![],
            vars: BTreeMap::from([("label".to_string(), "two words; rm -rf /".to_string())]),
            enabled: true,
            origin: Default::default(),
        }
    }

    fn check(command: &str, shell: bool) -> Check {
        let mut c = Check::for_tests("disk");
        c.command = command.into();
        c.shell = shell;
        c
    }

    #[test]
    fn argv_mode_keeps_values_as_one_argument() {
        let cmd = expand(
            &check("check_ping -H $host.address$ -l '$host.vars.label$'", false),
            &host(),
        )
        .unwrap();
        assert_eq!(
            cmd,
            CommandLine::Argv(vec![
                "check_ping".into(),
                "-H".into(),
                "10.0.0.5".into(),
                "-l".into(),
                "two words; rm -rf /".into()
            ])
        );
    }

    #[test]
    fn shell_mode_quotes_values() {
        let cmd = expand(
            &check("echo $host.vars.label$ $HOSTNAME$ $HOME $$ $1$", true),
            &host(),
        )
        .unwrap();
        assert_eq!(
            cmd,
            CommandLine::Shell("echo 'two words; rm -rf /' web01 $HOME $$ $1$".into())
        );
    }

    #[test]
    fn missing_and_unknown_macros_are_errors() {
        let err = expand(&check("x $host.vars.port$", false), &host()).unwrap_err();
        assert_eq!(err, "$host.vars.port$ is not set for host \"web01\"");
        let err = expand(&check("x $host.adress$", false), &host()).unwrap_err();
        assert_eq!(err, "unknown macro $host.adress$");
    }

    #[test]
    fn known_macros() {
        for name in [
            "host.name",
            "host.address",
            "host.node",
            "host.vars.x",
            "check.name",
            "check.vars.y",
            "HOSTNAME",
        ] {
            assert!(is_known(name), "{name}");
        }
        for name in ["host.adress", "host.vars", "host.vars.a.b", "check.address"] {
            assert!(!is_known(name), "{name}");
        }
    }

    #[test]
    fn lists_macro_names() {
        assert_eq!(
            names("a $host.name$ $X$ $check.vars.y$b"),
            ["host.name", "check.vars.y"]
        );
    }
}
