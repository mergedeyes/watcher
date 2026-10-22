//! Command macros: `$host.address$`, `$host.vars.port$`, `$service.name$`,
//! and bare vars like `$warn$`.
//!
//! Macros are `$host.…$`, `$service.…$`, `$check.…$`, the Nagios forms
//! `$HOSTNAME$` and `$HOSTADDRESS$`, and `$name$` with a lowercase var name
//! (`[a-z_][a-z0-9_]*`). Any other `$` is left alone, so shell variables
//! like `$HOME`, `$1` or `$(cmd)` keep working.
//!
//! Values are substituted after the command is split into arguments, so a
//! value with spaces stays one argument. In shell mode values are quoted for
//! `/bin/sh` instead; don't wrap macros in quotes there.

use watcher_exec::CommandLine;

/// Expands the macros in `command`; `lookup` gives the value of a macro
/// name (without the `$`s) or an error.
pub fn expand(
    command: &str,
    shell: bool,
    lookup: impl Fn(&str) -> Result<String, String>,
) -> Result<CommandLine, String> {
    if shell {
        let script = substitute(command, |name| {
            lookup(name).map(|v| shell_words::quote(&v).into_owned())
        })?;
        Ok(CommandLine::Shell(script))
    } else {
        let argv = shell_words::split(command).map_err(|e| format!("cannot split command: {e}"))?;
        let argv = argv
            .iter()
            .map(|arg| substitute(arg, &lookup))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(CommandLine::Argv(argv))
    }
}

/// A fixed macro (not a var), if `name` is one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fixed<'a> {
    HostName,
    HostAddress,
    HostNode,
    HostVar(&'a str),
    ServiceName,
    CheckName,
}

/// What a macro name refers to: a fixed macro, a var, or nothing known.
pub fn parse(name: &str) -> Option<Result<Fixed<'_>, &str>> {
    let fixed = match name.split('.').collect::<Vec<_>>().as_slice() {
        ["host", "name"] | ["HOSTNAME"] => Fixed::HostName,
        ["host", "address"] | ["HOSTADDRESS"] => Fixed::HostAddress,
        ["host", "node"] => Fixed::HostNode,
        ["host", "vars", var] => Fixed::HostVar(var),
        ["service", "name"] => Fixed::ServiceName,
        ["check", "name"] => Fixed::CheckName,
        [var] if is_var_name(var) => return Some(Err(var)),
        _ => return None,
    };
    Some(Ok(fixed))
}

/// Lists the macros used in `text`, for validation.
pub fn names(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some((_, name, after)) = next_macro(rest) {
        out.push(name);
        rest = after;
    }
    out
}

fn is_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
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
            || name.starts_with("service.")
            || name.starts_with("check.")
            || name == "HOSTNAME"
            || name == "HOSTADDRESS"
            || is_var_name(name);
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

    fn lookup(name: &str) -> Result<String, String> {
        match parse(name) {
            Some(Ok(Fixed::HostAddress)) => Ok("10.0.0.5".into()),
            Some(Ok(Fixed::HostName)) => Ok("web01".into()),
            Some(Ok(Fixed::HostVar("label"))) | Some(Err("label")) => {
                Ok("two words; rm -rf /".into())
            }
            Some(_) => Err(format!("${name}$ is not set")),
            None => Err(format!("unknown macro ${name}$")),
        }
    }

    #[test]
    fn argv_mode_keeps_values_as_one_argument() {
        let cmd = expand("check_ping -H $host.address$ -l '$label$'", false, lookup).unwrap();
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
            "echo $host.vars.label$ $HOSTNAME$ $HOME $$ $1$ $((1 + 2)) $(date)",
            true,
            lookup,
        )
        .unwrap();
        assert_eq!(
            cmd,
            CommandLine::Shell(
                "echo 'two words; rm -rf /' web01 $HOME $$ $1$ $((1 + 2)) $(date)".into()
            )
        );
    }

    #[test]
    fn missing_and_unknown_macros_are_errors() {
        assert_eq!(
            expand("x $port$", false, lookup).unwrap_err(),
            "$port$ is not set"
        );
        assert_eq!(
            expand("x $host.adress$", false, lookup).unwrap_err(),
            "unknown macro $host.adress$"
        );
    }

    #[test]
    fn parses_macro_names() {
        assert_eq!(parse("host.vars.x"), Some(Ok(Fixed::HostVar("x"))));
        assert_eq!(parse("service.name"), Some(Ok(Fixed::ServiceName)));
        assert_eq!(parse("warn"), Some(Err("warn")));
        assert_eq!(parse("host.adress"), None);
        assert_eq!(parse("host.vars.a.b"), None);
        assert_eq!(parse("Warn"), None);
    }

    #[test]
    fn lists_macro_names() {
        assert_eq!(
            names("a $host.name$ $X$ $warn$ $service.name$b"),
            ["host.name", "warn", "service.name"]
        );
    }
}
