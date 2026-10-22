use super::ParseError;
use crate::metric::Metric;
use crate::num::split_number;
use crate::unit::Unit;
use std::collections::BTreeMap;

/// Parses `key=value[unit]` output, e.g. `latency=12ms temp{sensor="cpu0"}=41`.
///
/// Every non-empty line must consist only of such tokens; this is what makes
/// the format safe to auto-detect.
pub(super) fn parse(text: &str) -> Result<Vec<Metric>, ParseError> {
    let mut metrics = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let mut rest = line.trim();
        while !rest.is_empty() {
            let (metric, tail) =
                token(rest).map_err(|msg| ParseError::KeyValue { line: idx + 1, msg })?;
            metrics.push(metric);
            rest = tail.trim_start();
        }
    }
    Ok(metrics)
}

/// Parses one token from the start of `s` and returns it with the remainder.
fn token(s: &str) -> Result<(Metric, &str), String> {
    let key_len = s.find(|c: char| !is_key_char(c)).unwrap_or(s.len());
    let key = &s[..key_len];
    if !key.starts_with(|c: char| c.is_alphabetic() || c == '_') {
        let end = s.find(char::is_whitespace).unwrap_or(s.len());
        return Err(format!("expected key=value, found {:?}", &s[..end]));
    }

    let mut rest = &s[key_len..];
    let mut labels = BTreeMap::new();
    if let Some(inner) = rest.strip_prefix('{') {
        rest = parse_labels(inner, &mut labels).map_err(|e| format!("{key}: {e}"))?;
    }
    let rest = rest
        .strip_prefix('=')
        .ok_or_else(|| format!("expected '=' after {key:?}"))?;

    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let (raw, tail) = rest.split_at(end);
    let invalid = || format!("{key}: invalid value {raw:?}");
    let (value, unit) = split_number(raw).ok_or_else(invalid)?;
    if !is_unit(unit) {
        return Err(invalid());
    }
    let (unit, factor) = Unit::parse(unit);

    let mut metric = Metric::new(key, value * factor).with_unit(unit);
    metric.labels = labels;
    Ok((metric, tail))
}

/// Parses `name="value", …}` (opening brace already stripped) and returns
/// what follows the closing brace.
fn parse_labels<'a>(
    mut s: &'a str,
    labels: &mut BTreeMap<String, String>,
) -> Result<&'a str, String> {
    loop {
        s = s.trim_start();
        if let Some(rest) = s.strip_prefix('}') {
            return Ok(rest);
        }
        let name_len = s
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(s.len());
        if name_len == 0 {
            return Err("invalid label name".into());
        }
        let name = &s[..name_len];
        s = s[name_len..]
            .trim_start()
            .strip_prefix('=')
            .and_then(|r| r.trim_start().strip_prefix('"'))
            .ok_or_else(|| format!("label {name:?} needs a quoted value"))?;

        let mut value = String::new();
        let mut chars = s.char_indices();
        let end = loop {
            match chars.next() {
                None => return Err(format!("unterminated value for label {name:?}")),
                Some((i, '"')) => break i + 1,
                Some((_, '\\')) => match chars.next() {
                    Some((_, 'n')) => value.push('\n'),
                    Some((_, c)) => value.push(c),
                    None => return Err(format!("unterminated value for label {name:?}")),
                },
                Some((_, c)) => value.push(c),
            }
        };
        labels.insert(name.to_string(), value);

        s = s[end..].trim_start();
        if let Some(rest) = s.strip_prefix(',') {
            s = rest;
        } else if !s.starts_with('}') {
            return Err("expected ',' or '}' after label".into());
        }
    }
}

fn is_key_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '.' | ':' | '-' | '/')
}

/// Units are letters plus a few symbols (`ms`, `°C`, `req/s`, `%`). This
/// rejects things like `1;2;3` or `1,2,3` that would otherwise slip through.
fn is_unit(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_alphabetic() || matches!(c, '%' | '/' | '_' | '°'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_and_tokens() {
        let metrics = parse("depth=42\n\nlatency=12ms errors=0\n").unwrap();
        let got: Vec<_> = metrics.iter().map(|m| (m.name.as_str(), m.value)).collect();
        assert_eq!(got, [("depth", 42.0), ("latency", 0.012), ("errors", 0.0)]);
        assert_eq!(metrics[1].unit, Unit::Seconds);
    }

    #[test]
    fn prometheus_labels() {
        let metrics =
            parse(r#"temp{sensor="cpu 0", zone="a\"b"}=41°C fan{id="1",}=1200rpm"#).unwrap();
        assert_eq!(metrics[0].labels["sensor"], "cpu 0");
        assert_eq!(metrics[0].labels["zone"], "a\"b");
        assert_eq!(metrics[0].unit, Unit::Other("°C".into()));
        assert_eq!(metrics[1].labels["id"], "1");
        assert_eq!(metrics[1].value, 1200.0);
    }

    #[test]
    fn rejects_non_key_value_lines() {
        for text in [
            "OK - all good",
            "depth=42 and more",
            "a=1;2;3",
            "a=1,2,3",
            "a=",
            "1a=2",
            "a=x",
            r#"a{b=c}=1"#,
            r#"a{b="c"=1"#,
            "OK | a=1",
        ] {
            assert!(parse(text).is_err(), "{text}");
        }
        assert_eq!(
            parse("a=1\nfoo").unwrap_err(),
            ParseError::KeyValue {
                line: 2,
                msg: "expected '=' after \"foo\"".into()
            }
        );
    }
}
