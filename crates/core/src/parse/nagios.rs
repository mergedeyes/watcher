use super::{Format, ParseError, ParsedOutput};
use crate::metric::Metric;
use crate::num::{parse_number, split_number};
use crate::threshold::Threshold;
use crate::unit::Unit;

/// Parses Nagios plugin output:
///
/// ```text
/// STATUS TEXT | perfdata
/// long output line 1
/// long output line 2 | more perfdata
/// more perfdata
/// ```
pub(super) fn parse(text: &str) -> Result<ParsedOutput, ParseError> {
    let text = text.trim_end();
    let (first, rest) = text.split_once('\n').unwrap_or((text, ""));
    let (message, perf) = first.split_once('|').unwrap_or((first, ""));
    let (long_output, long_perf) = rest.split_once('|').unwrap_or((rest, ""));

    let mut metrics = perfdata(perf)?;
    metrics.extend(perfdata(long_perf)?);

    Ok(ParsedOutput {
        format: Format::Nagios,
        state: None,
        message: message.trim().to_string(),
        long_output: long_output.trim_end().to_string(),
        metrics,
    })
}

/// Parses space-separated `'label'=value[UOM];[warn];[crit];[min];[max]` items.
fn perfdata(s: &str) -> Result<Vec<Metric>, ParseError> {
    let mut metrics = Vec::new();
    let mut rest = s.trim_start();
    while !rest.is_empty() {
        let token_start = rest;
        let fail = |rest: &str, msg: &str| {
            let len = token_start.len() - rest.len();
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            ParseError::Perfdata {
                token: token_start[..len + end].to_string(),
                msg: msg.to_string(),
            }
        };

        let (label, after) = match rest.strip_prefix('\'') {
            Some(quoted) => split_quoted(quoted).ok_or_else(|| fail(rest, "unterminated quote"))?,
            None => {
                let end = rest
                    .find(|c: char| c == '=' || c.is_whitespace())
                    .unwrap_or(rest.len());
                (rest[..end].to_string(), &rest[end..])
            }
        };
        if label.is_empty() {
            return Err(fail(after, "empty label"));
        }
        let after = after
            .strip_prefix('=')
            .ok_or_else(|| fail(after, "expected '='"))?;
        let end = after.find(char::is_whitespace).unwrap_or(after.len());
        let (spec, tail) = after.split_at(end);
        if let Some(metric) = perf_metric(label, spec).map_err(|msg| fail(tail, msg))? {
            metrics.push(metric);
        }
        rest = tail.trim_start();
    }
    Ok(metrics)
}

/// Splits a single-quoted label (opening quote already stripped) from the
/// rest. `''` inside the label stands for a literal quote.
fn split_quoted(s: &str) -> Option<(String, &str)> {
    let mut label = String::new();
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c != '\'' {
            label.push(c);
        } else if chars.next_if(|&(_, c)| c == '\'').is_some() {
            label.push('\'');
        } else {
            return Some((label, &s[i + 1..]));
        }
    }
    None
}

/// Parses `value[UOM];warn;crit;min;max`. Returns `None` for the value `U`
/// ("undetermined"), which carries no data.
fn perf_metric(label: String, spec: &str) -> Result<Option<Metric>, &'static str> {
    let mut fields = spec.split(';');
    let value = fields.next().unwrap_or_default();
    if value == "U" {
        return Ok(None);
    }
    let (value, uom) = split_number(value).ok_or("invalid value")?;
    let (unit, factor) = Unit::parse(uom);

    let mut field = || fields.next().map(str::trim).filter(|f| !f.is_empty());
    let threshold = |f: Option<&str>| {
        f.map(|f| Threshold::parse_scaled(f, factor))
            .transpose()
            .map_err(|_| "invalid threshold")
    };
    let bound = |f: Option<&str>| {
        f.map(|f| parse_number(f).map(|v| v * factor).ok_or("invalid min/max"))
            .transpose()
    };
    let warn = threshold(field())?;
    let crit = threshold(field())?;
    let min = bound(field())?;
    let max = bound(field())?;

    let mut metric = Metric::new(label, value * factor).with_unit(unit);
    metric.warn = warn;
    metric.crit = crit;
    metric.min = min;
    metric.max = max;
    Ok(Some(metric))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unit::Unit;

    const MB: f64 = 1024.0 * 1024.0;

    #[test]
    fn status_line_with_perfdata() {
        let out =
            parse("DISK OK - free space: / 3326 MB (56%) | /=2643MB;5948;5958;0;5968\n").unwrap();
        assert_eq!(out.message, "DISK OK - free space: / 3326 MB (56%)");
        assert_eq!(out.long_output, "");
        let [m] = out.metrics.as_slice() else {
            panic!("{:?}", out.metrics)
        };
        assert_eq!(m.name, "/");
        assert_eq!(m.value, 2643.0 * MB);
        assert_eq!(m.unit, Unit::Bytes);
        assert_eq!(
            m.warn,
            Some(Threshold::Range {
                lo: 0.0,
                hi: 5948.0 * MB,
                inside: false
            })
        );
        assert_eq!(
            m.crit,
            Some(Threshold::Range {
                lo: 0.0,
                hi: 5958.0 * MB,
                inside: false
            })
        );
        assert_eq!(m.min, Some(0.0));
        assert_eq!(m.max, Some(5968.0 * MB));
    }

    #[test]
    fn long_output_and_extra_perfdata() {
        let text = "\
DISK OK - free space: / 3326 MB (56%); | /=2643MB;5948;5958;0;5968
/ 15272 MB (77%);
/boot 68 MB (69%);
/home 69357 MB (27%);
/var/log 819 MB (84%); | /boot=68MB;88;93;0;98
/home=69357MB;253404;253409;0;253414
/var/log=818MB;970;975;0;980
";
        let out = parse(text).unwrap();
        assert_eq!(out.message, "DISK OK - free space: / 3326 MB (56%);");
        assert_eq!(
            out.long_output,
            "/ 15272 MB (77%);\n/boot 68 MB (69%);\n/home 69357 MB (27%);\n/var/log 819 MB (84%);"
        );
        let names: Vec<_> = out.metrics.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["/", "/boot", "/home", "/var/log"]);
    }

    #[test]
    fn quoted_labels_units_and_partial_fields() {
        let out = parse(
            "OK | 'used space'=1.5GB;; 'it''s'=3 time=12ms;@10:20 pct=56%;80;90 c=5c U=U x=U;1;2",
        )
        .unwrap();
        let get = |n: &str| out.metrics.iter().find(|m| m.name == n).unwrap();
        assert_eq!(get("used space").value, 1.5 * 1024.0 * MB);
        assert_eq!(get("used space").warn, None);
        assert_eq!(get("it's").value, 3.0);
        assert_eq!(get("time").value, 0.012);
        assert_eq!(
            get("time").warn,
            Some(Threshold::Range {
                lo: 0.01,
                hi: 0.02,
                inside: true
            })
        );
        assert_eq!(get("pct").unit, Unit::Percent);
        assert_eq!(get("c").unit, Unit::Counter);
        // `U` values carry no data and are dropped, whatever the label.
        assert_eq!(out.metrics.len(), 5);
    }

    #[test]
    fn comma_decimal_values() {
        let out = parse("OK | load1=0,52;5,0;10,0;0").unwrap();
        assert_eq!(out.metrics[0].value, 0.52);
    }

    #[test]
    fn plain_text_without_perfdata() {
        let out = parse("PING OK - Packet loss = 0%").unwrap();
        assert_eq!(out.message, "PING OK - Packet loss = 0%");
        assert!(out.metrics.is_empty());
    }

    #[test]
    fn rejects_malformed_perfdata() {
        for text in [
            "OK | load",
            "OK | load=abc",
            "OK | 'load=1",
            "OK | =1",
            "OK | a=1;x",
        ] {
            assert!(
                matches!(parse(text), Err(ParseError::Perfdata { .. })),
                "{text}"
            );
        }
        let err = parse("OK | a=1 b=1;oops c=2").unwrap_err();
        assert_eq!(
            err,
            ParseError::Perfdata {
                token: "b=1;oops".into(),
                msg: "invalid threshold".into()
            }
        );
    }
}
