//! Time-based one-time passwords as RFC 6238 has them: HMAC-SHA1 of the 30-second step since
//! the Unix epoch, truncated as RFC 4226 says, six digits. What every authenticator app makes.

use hmac::{Hmac, KeyInit, Mac};
use sha1::Sha1;

/// Seconds each code lasts.
pub const STEP: i64 = 30;
/// Digits in a code.
pub const DIGITS: u32 = 6;
/// Steps either side of now a code is still taken for, for a clock a little off and a code
/// typed as it turned over.
const WINDOW: i64 = 1;

/// The step `unix` seconds fall in.
pub fn step_at(unix: i64) -> i64 {
    unix.div_euclid(STEP)
}

/// The HOTP value for `counter` (RFC 4226), `digits` long.
pub(super) fn hotp(secret: &[u8], counter: u64, digits: u32) -> u32 {
    let mut mac = Hmac::<Sha1>::new_from_slice(secret).expect("HMAC takes a key of any length");
    mac.update(&counter.to_be_bytes());
    let hash = mac.finalize().into_bytes();
    let offset = (hash[hash.len() - 1] & 0x0f) as usize;
    let binary = u32::from_be_bytes([
        hash[offset] & 0x7f,
        hash[offset + 1],
        hash[offset + 2],
        hash[offset + 3],
    ]);
    binary % 10u32.pow(digits)
}

/// The six-digit code for `step`, zero-padded as an app shows it.
pub fn code_at(secret: &[u8], step: i64) -> String {
    let counter = u64::try_from(step).unwrap_or(0);
    format!(
        "{:0width$}",
        hotp(secret, counter, DIGITS),
        width = DIGITS as usize
    )
}

/// The step `code` was made for, when it's the code of a step within the window around `now`.
/// Every step in the window is checked, and each comparison takes as long whether it matches
/// or not, so the time taken says nothing about the code.
pub fn matching_step(secret: &[u8], code: &str, now: i64) -> Option<i64> {
    let code = code.trim();
    if code.len() != DIGITS as usize || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let current = step_at(now);
    let mut found = None;
    for step in current - WINDOW..=current + WINDOW {
        if same(code_at(secret, step).as_bytes(), code.as_bytes()) && found.is_none() {
            found = Some(step);
        }
    }
    found
}

/// Equal byte strings of the same length, compared in a time that doesn't depend on where
/// they differ.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

/// `bytes` in base32 (RFC 4648) without padding, as authenticator apps take a key.
pub fn base32(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for byte in bytes {
        buffer = (buffer << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The raw secret behind a base32 one, as an authenticator app reads it.
    pub(crate) fn from_base32(base32: &str) -> Vec<u8> {
        const ALPHABET: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
        let (mut out, mut buffer, mut bits) = (Vec::new(), 0u32, 0u32);
        for c in base32.chars() {
            buffer = (buffer << 5) | ALPHABET.find(c).expect("a base32 letter") as u32;
            bits += 5;
            if bits >= 8 {
                bits -= 8;
                out.push((buffer >> bits) as u8);
            }
        }
        out
    }

    /// RFC 6238's key for its SHA-1 vectors.
    const RFC_KEY: &[u8] = b"12345678901234567890";

    #[test]
    fn rfc_6238_sha1_vectors() {
        // Appendix B: time, then the 8-digit TOTP for SHA-1 with a 30-second step.
        let vectors = [
            (59, 94287082),
            (1111111109, 7081804),
            (1111111111, 14050471),
            (1234567890, 89005924),
            (2000000000, 69279037),
            (20000000000, 65353130),
        ];
        for (time, expected) in vectors {
            let counter = u64::try_from(step_at(time)).unwrap();
            assert_eq!(hotp(RFC_KEY, counter, 8), expected, "at {time}");
        }
    }

    #[test]
    fn rfc_4226_six_digit_vectors() {
        // Appendix D: counters 0 to 9, the six digits an app would show.
        let expected = [
            "755224", "287082", "359152", "969429", "338314", "254676", "287922", "162583",
            "399871", "520489",
        ];
        for (step, code) in expected.iter().enumerate() {
            assert_eq!(code_at(RFC_KEY, step as i64), *code);
        }
        assert_eq!(code_at(RFC_KEY, step_at(59)), "287082", "zero-padded");
    }

    #[test]
    fn a_code_is_taken_one_step_either_side_and_no_further() {
        let now = 1111111111;
        let step = step_at(now);
        for offset in -1..=1 {
            let code = code_at(RFC_KEY, step + offset);
            assert_eq!(matching_step(RFC_KEY, &code, now), Some(step + offset));
        }
        for offset in [-2, 2] {
            let code = code_at(RFC_KEY, step + offset);
            assert_eq!(matching_step(RFC_KEY, &code, now), None, "{offset}");
        }
        let code = code_at(RFC_KEY, step);
        assert_eq!(
            matching_step(RFC_KEY, &format!(" {code} "), now),
            Some(step)
        );
        for bad in ["", "12345", "1234567", "12345a", "١٢٣٤٥٦"] {
            assert_eq!(matching_step(RFC_KEY, bad, now), None, "{bad}");
        }
        assert_eq!(matching_step(b"another key here", &code, now), None);
    }

    #[test]
    fn base32_is_rfc_4648_without_padding() {
        // RFC 4648, section 10, padding taken off.
        for (plain, encoded) in [
            ("", ""),
            ("f", "MY"),
            ("fo", "MZXQ"),
            ("foo", "MZXW6"),
            ("foob", "MZXW6YQ"),
            ("fooba", "MZXW6YTB"),
            ("foobar", "MZXW6YTBOI"),
        ] {
            assert_eq!(base32(plain.as_bytes()), encoded);
        }
        assert_eq!(base32(RFC_KEY), "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ");
    }
}
