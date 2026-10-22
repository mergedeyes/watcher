/// Splits a leading decimal number off `s` and returns it with the remainder.
///
/// A `,` is accepted as decimal separator because plugins running under
/// German and other European locales print `0,5`. Thousands separators are
/// not supported (the Nagios perfdata spec does not allow them either).
pub(crate) fn split_number(s: &str) -> Option<(f64, &str)> {
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let mut seen_digit = false;
    let mut seen_sep = false;
    while i < b.len() {
        match b[i] {
            b'0'..=b'9' => seen_digit = true,
            b'.' | b',' if !seen_sep => seen_sep = true,
            _ => break,
        }
        i += 1;
    }
    if !seen_digit {
        return None;
    }
    // Only treat `e` as an exponent when digits follow, so units like `events` survive.
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let digits = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > digits {
            i = j;
        }
    }
    let value = s[..i].replace(',', ".").parse().ok()?;
    Some((value, &s[i..]))
}

/// Parses `s` as a number, rejecting anything after it.
pub(crate) fn parse_number(s: &str) -> Option<f64> {
    match split_number(s.trim()) {
        Some((value, "")) => Some(value),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_numbers_and_units() {
        assert_eq!(split_number("42"), Some((42.0, "")));
        assert_eq!(split_number("-1.5ms"), Some((-1.5, "ms")));
        assert_eq!(split_number("0,25s"), Some((0.25, "s")));
        assert_eq!(split_number(".5"), Some((0.5, "")));
        assert_eq!(split_number("1e3B"), Some((1000.0, "B")));
        assert_eq!(split_number("3events"), Some((3.0, "events")));
        assert_eq!(split_number("U"), None);
        assert_eq!(split_number("-"), None);
    }

    #[test]
    fn parses_whole_numbers_only() {
        assert_eq!(parse_number(" 10 "), Some(10.0));
        assert_eq!(parse_number("10x"), None);
        assert_eq!(parse_number(""), None);
    }
}
