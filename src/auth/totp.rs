//! RFC 6238 TOTP (SHA-1, 6 digits, 30 s) and recovery codes.
//!
//! Source `packages/core/src/mfa.ts`. Codes are compared in constant time and
//! the matching step is returned so the caller can claim it (replay block).

use rand::RngCore;
use ring::hmac;

pub const TOTP_STEP_SECONDS: i64 = 30;
pub const TOTP_DIGITS: usize = 6;
pub const RECOVERY_CODE_COUNT: usize = 10;
/// 60 bits: recovery codes are stored as plain sha256, so a DB leak must not
/// make them cheap to guess offline (source comment).
pub const RECOVERY_CODE_LENGTH: usize = 12;
pub const SECRET_BYTES: usize = 20;

const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

pub fn base32_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let mut bits = 0u32;
    let mut value = 0u32;
    for &byte in bytes {
        value = (value << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            out.push(BASE32[((value >> (bits - 5)) & 31) as usize] as char);
            bits -= 5;
        }
        value &= (1 << bits) - 1;
    }
    if bits > 0 {
        out.push(BASE32[((value << (5 - bits)) & 31) as usize] as char);
    }
    out
}

pub fn totp_step(now_ms: i64) -> i64 {
    now_ms.div_euclid(1000).div_euclid(TOTP_STEP_SECONDS)
}

pub fn totp_code(secret: &[u8], step: i64) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, secret);
    let mac = hmac::sign(&key, &(step as u64).to_be_bytes());
    let mac = mac.as_ref();
    let offset = (mac[19] & 0x0f) as usize;
    let bin = (u32::from(mac[offset] & 0x7f) << 24)
        | (u32::from(mac[offset + 1]) << 16)
        | (u32::from(mac[offset + 2]) << 8)
        | u32::from(mac[offset + 3]);
    format!("{:06}", bin % 1_000_000)
}

pub fn is_totp_shape(code: &str) -> bool {
    code.len() == TOTP_DIGITS && code.bytes().all(|b| b.is_ascii_digit())
}

/// Constant-time equality for equal-length ASCII codes.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// The step (now, now-1, now+1) whose code matches, or None. Every candidate
/// is computed and compared so timing does not reveal which step matched.
pub fn match_totp(secret: &[u8], code: &str, now_ms: i64) -> Option<i64> {
    if !is_totp_shape(code) {
        return None;
    }
    let now = totp_step(now_ms);
    let mut found = None;
    for step in [now, now - 1, now + 1] {
        if ct_eq(totp_code(secret, step).as_bytes(), code.as_bytes()) && found.is_none() {
            found = Some(step);
        }
    }
    found
}

pub fn new_secret() -> [u8; SECRET_BYTES] {
    let mut raw = [0u8; SECRET_BYTES];
    rand::rng().fill_bytes(&mut raw);
    raw
}

/// Ten 12-character lowercase base32 codes.
pub fn new_recovery_codes() -> Vec<String> {
    (0..RECOVERY_CODE_COUNT)
        .map(|_| {
            let mut raw = [0u8; 8];
            rand::rng().fill_bytes(&mut raw);
            base32_encode(&raw)[..RECOVERY_CODE_LENGTH].to_ascii_lowercase()
        })
        .collect()
}

/// Display form `xxxx-xxxx-xxxx`.
pub fn format_recovery_code(code: &str) -> String {
    code.as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("-")
}

/// Source `normalizeRecoveryCode`: lowercase, keep only base32 characters.
pub fn normalize_recovery_code(code: &str) -> String {
    code.to_lowercase()
        .chars()
        .filter(|c| matches!(c, 'a'..='z' | '2'..='7'))
        .collect()
}

pub fn otpauth_uri(issuer: &str, email: &str, secret_b32: &str) -> String {
    let label = url::form_urlencoded::byte_serialize(format!("{issuer}:{email}").as_bytes())
        .collect::<String>()
        .replace('+', "%20");
    let params = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("secret", secret_b32)
        .append_pair("issuer", issuer)
        .append_pair("algorithm", "SHA1")
        .append_pair("digits", &TOTP_DIGITS.to_string())
        .append_pair("period", &TOTP_STEP_SECONDS.to_string())
        .finish();
    format!("otpauth://totp/{label}?{params}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const RFC_SECRET: &[u8] = b"12345678901234567890";

    #[test]
    fn rfc6238_sha1_vectors_truncated_to_six_digits() {
        // RFC 6238 appendix B (8-digit values, last six digits kept).
        for (time, expected) in [
            (59i64, "287082"),
            (1_111_111_109, "081804"),
            (1_111_111_111, "050471"),
            (1_234_567_890, "005924"),
            (2_000_000_000, "279037"),
            (20_000_000_000, "353130"),
        ] {
            assert_eq!(totp_code(RFC_SECRET, totp_step(time * 1000)), expected);
        }
    }

    #[test]
    fn match_accepts_adjacent_steps_only() {
        let now_ms = 1_111_111_111_000;
        let step = totp_step(now_ms);
        assert_eq!(
            match_totp(RFC_SECRET, &totp_code(RFC_SECRET, step), now_ms),
            Some(step)
        );
        assert_eq!(
            match_totp(RFC_SECRET, &totp_code(RFC_SECRET, step - 1), now_ms),
            Some(step - 1)
        );
        assert_eq!(
            match_totp(RFC_SECRET, &totp_code(RFC_SECRET, step + 1), now_ms),
            Some(step + 1)
        );
        let far = totp_code(RFC_SECRET, step - 2);
        if far != totp_code(RFC_SECRET, step)
            && far != totp_code(RFC_SECRET, step - 1)
            && far != totp_code(RFC_SECRET, step + 1)
        {
            assert_eq!(match_totp(RFC_SECRET, &far, now_ms), None);
        }
        assert_eq!(match_totp(RFC_SECRET, "12345", now_ms), None);
        assert_eq!(match_totp(RFC_SECRET, "12345a", now_ms), None);
        assert_eq!(match_totp(RFC_SECRET, " 287082", now_ms), None);
    }

    #[test]
    fn base32_matches_rfc4648() {
        assert_eq!(base32_encode(b""), "");
        assert_eq!(base32_encode(b"f"), "MY");
        assert_eq!(base32_encode(b"fo"), "MZXQ");
        assert_eq!(base32_encode(b"foo"), "MZXW6");
        assert_eq!(base32_encode(b"foobar"), "MZXW6YTBOI");
        assert_eq!(base32_encode(&[0xff; 20]).len(), 32);
    }

    #[test]
    fn recovery_codes_shape_and_normalization() {
        let codes = new_recovery_codes();
        assert_eq!(codes.len(), RECOVERY_CODE_COUNT);
        for code in &codes {
            assert_eq!(code.len(), RECOVERY_CODE_LENGTH);
            assert!(code.chars().all(|c| matches!(c, 'a'..='z' | '2'..='7')));
            let shown = format_recovery_code(code);
            assert_eq!(shown.len(), 14);
            assert_eq!(normalize_recovery_code(&shown.to_uppercase()), *code);
        }
        assert_eq!(normalize_recovery_code(" AB-cd 18 "), "abcd");
    }

    #[test]
    fn otpauth_uri_escapes_label() {
        let uri = otpauth_uri("fvoci.example", "kim@example.com", "ABC");
        assert_eq!(
            uri,
            "otpauth://totp/fvoci.example%3Akim%40example.com?secret=ABC&issuer=fvoci.example&algorithm=SHA1&digits=6&period=30"
        );
    }
}
