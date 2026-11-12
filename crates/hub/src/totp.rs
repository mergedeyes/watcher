//! Time-based one-time codes for two-factor authentication (TOTP,
//! RFC 6238): HMAC-SHA1, six digits, 30-second steps. Every authenticator
//! app supports these parameters, and many ignore any others.

use data_encoding::BASE32_NOPAD;
use ring::hmac;
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

const DIGITS: usize = 6;
/// Seconds per time step.
const STEP: u64 = 30;
/// 160 bits, the length RFC 4226 recommends for HMAC-SHA1.
const SECRET_LEN: usize = 20;
/// Recovery codes a user gets at a time.
const RECOVERY_CODES: usize = 10;
/// Letters and digits of recovery codes, without the ones that are easy to
/// mistake for each other (0/o, 1/l). 32 symbols: 5 bits each.
const ALPHABET: &[u8; 32] = b"abcdefghijkmnpqrstuvwxyz23456789";

/// A new random secret.
pub fn new_secret() -> Vec<u8> {
    let mut secret = vec![0u8; SECRET_LEN];
    getrandom::fill(&mut secret).expect("the OS random number generator works");
    secret
}

/// The secret as people type it into an app: base32 without padding.
pub fn encode(secret: &[u8]) -> String {
    BASE32_NOPAD.encode(secret)
}

/// A secret written by [`encode`]. Spaces and lowercase letters are fine,
/// as apps show the secret in groups. At least 128 bits.
pub fn decode(text: &str) -> Option<Vec<u8>> {
    let clean: String = text
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '=')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    BASE32_NOPAD
        .decode(clean.as_bytes())
        .ok()
        .filter(|s| s.len() >= 16)
}

/// The current time step.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
        / STEP
}

/// The code for time step `step`.
fn code_at(secret: &[u8], step: u64) -> u32 {
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, secret);
    let tag = hmac::sign(&key, &step.to_be_bytes());
    let h = tag.as_ref();
    // Dynamic truncation (RFC 4226, 5.3).
    let at = usize::from(h[h.len() - 1] & 0x0f);
    let bits = u32::from_be_bytes([h[at] & 0x7f, h[at + 1], h[at + 2], h[at + 3]]);
    bits % 10u32.pow(DIGITS as u32)
}

/// The code an app shows at time step `step`.
pub fn code(secret: &[u8], step: u64) -> String {
    format!("{:0DIGITS$}", code_at(secret, step))
}

fn compact(code: &str) -> String {
    code.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Whether `text` looks like an app code (six digits) rather than a
/// recovery code.
pub fn is_code(text: &str) -> bool {
    let code = compact(text);
    code.len() == DIGITS && code.bytes().all(|b| b.is_ascii_digit())
}

/// The time step of `code` if it is valid at step `now`. Codes of the step
/// before and after count too: phone clocks are a little off at times, and
/// a code typed at the end of its step arrives in the next one.
pub fn verify(secret: &[u8], code: &str, now: u64) -> Option<u64> {
    if !is_code(code) {
        return None;
    }
    let code: u32 = compact(code).parse().ok()?;
    [now.saturating_sub(1), now, now + 1]
        .into_iter()
        .find(|&step| code_at(secret, step) == code)
}

/// The `otpauth://` URI that authenticator apps read from the QR code.
pub fn uri(issuer: &str, account: &str, secret: &[u8]) -> String {
    format!(
        "otpauth://totp/{}:{}?secret={}&issuer={}&algorithm=SHA1&digits={DIGITS}&period={STEP}",
        percent(issuer),
        percent(account),
        encode(secret),
        percent(issuer),
    )
}

/// Percent-encodes everything but unreserved characters (RFC 3986).
fn percent(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(b));
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `uri` as a QR code, as an SVG image.
pub fn qr_svg(uri: &str) -> Result<String, String> {
    let code = qrcode::QrCode::new(uri.as_bytes()).map_err(|e| e.to_string())?;
    Ok(code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(200, 200)
        .build())
}

/// `uri` as a QR code for a terminal, two rows per line in Unicode half
/// blocks. Light modules are drawn, so it reads right on the usual dark
/// terminal background.
pub fn qr_text(uri: &str) -> Result<String, String> {
    use qrcode::render::unicode::Dense1x2;
    let code = qrcode::QrCode::new(uri.as_bytes()).map_err(|e| e.to_string())?;
    Ok(code
        .render::<Dense1x2>()
        .dark_color(Dense1x2::Light)
        .light_color(Dense1x2::Dark)
        .build())
}

/// New recovery codes, like `k7mqa-x3vbn` (50 bits each).
pub fn new_recovery_codes() -> Vec<String> {
    (0..RECOVERY_CODES)
        .map(|_| {
            let mut bytes = [0u8; 10];
            getrandom::fill(&mut bytes).expect("the OS random number generator works");
            // 256 is a multiple of 32, so every symbol is equally likely.
            let chars: String = bytes
                .iter()
                .map(|b| char::from(ALPHABET[usize::from(b & 31)]))
                .collect();
            format!("{}-{}", &chars[..5], &chars[5..])
        })
        .collect()
}

/// The hash stored for a recovery code. Case, spaces and dashes don't
/// matter, so codes may be typed as they come.
pub fn recovery_hash(code: &str) -> Vec<u8> {
    let clean: String = code
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    Sha256::digest(clean.as_bytes()).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The SHA-1 test vectors of RFC 6238, appendix B (last six digits).
    #[test]
    fn rfc_6238_vectors() {
        let secret = b"12345678901234567890";
        for (time, code) in [
            (59, 287_082),
            (1_111_111_109, 81_804),
            (1_111_111_111, 50_471),
            (1_234_567_890, 5_924),
            (2_000_000_000, 279_037),
            (20_000_000_000, 353_130),
        ] {
            assert_eq!(code_at(secret, time / STEP), code, "at {time}");
        }
    }

    #[test]
    fn verifies_neighbouring_steps_only() {
        let secret = new_secret();
        let now = 1_000_000;
        for step in [now - 1, now, now + 1] {
            let code = code(&secret, step);
            assert_eq!(verify(&secret, &code, now), Some(step));
            // Apps show codes as "123 456".
            let spaced = format!("{} {}", &code[..3], &code[3..]);
            assert_eq!(verify(&secret, &spaced, now), Some(step));
        }
        for step in [now - 3, now + 3] {
            assert_eq!(verify(&secret, &code(&secret, step), now), None);
        }
        assert_eq!(
            code(b"12345678901234567890", 1_234_567_890 / STEP),
            "005924"
        );
        assert_eq!(verify(&secret, "12345", now), None);
        assert_eq!(verify(&secret, "abcdef", now), None);
    }

    #[test]
    fn secrets_round_trip() {
        let secret = new_secret();
        let text = encode(&secret);
        assert_eq!(text.len(), 32);
        assert_eq!(decode(&text), Some(secret.clone()));
        let grouped = text
            .to_lowercase()
            .as_bytes()
            .chunks(4)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(decode(&grouped), Some(secret));
        assert_eq!(decode("JBSWY3DP"), None, "too short");
        assert_eq!(decode("not base32!"), None);
    }

    #[test]
    fn uri_for_apps() {
        let uri = uri("Watcher Prod", "alice@example", b"12345678901234567890");
        assert_eq!(
            uri,
            "otpauth://totp/Watcher%20Prod:alice%40example?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ\
             &issuer=Watcher%20Prod&algorithm=SHA1&digits=6&period=30"
        );
        let svg = qr_svg(&uri).unwrap();
        assert!(svg.contains("<svg"), "{svg}");
        let text = qr_text(&uri).unwrap();
        assert!(text.lines().count() > 20 && text.contains('█'), "{text}");
    }

    #[test]
    fn recovery_codes() {
        let codes = new_recovery_codes();
        assert_eq!(codes.len(), RECOVERY_CODES);
        for code in &codes {
            assert_eq!(code.len(), 11);
            assert_eq!(&code[5..6], "-");
            assert!(!is_code(code));
            assert!(code.bytes().all(|b| b == b'-' || ALPHABET.contains(&b)));
        }
        assert_ne!(codes[0], codes[1]);
        let hash = recovery_hash(&codes[0]);
        assert_eq!(recovery_hash(&codes[0].to_uppercase()), hash);
        assert_eq!(recovery_hash(&codes[0].replace('-', " ")), hash);
        assert_ne!(recovery_hash(&codes[1]), hash);
    }
}
