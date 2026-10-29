//! Random secrets, PKCE's S256 challenge, and the form encoding a client id and secret need.

use super::*;

/// A random secret an app holds while it signs in.
pub fn new_secret() -> anyhow::Result<String> {
    crate::auth::random_token(SECRET_PREFIX)
}

/// `bytes` random bytes, URL-safe.
pub fn random(bytes: usize) -> anyhow::Result<String> {
    let mut buffer = vec![0u8; bytes];
    getrandom::fill(&mut buffer).map_err(|e| anyhow!("no randomness available: {e}"))?;
    Ok(URL_SAFE_NO_PAD.encode(buffer))
}

/// PKCE's S256 challenge for a verifier.
pub(super) fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// application/x-www-form-urlencoded, as RFC 6749 wants a client id and secret before Basic.
pub(super) fn form_encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'*' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
