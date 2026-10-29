//! Rewriting one value in an op's payload, leaving every other byte as it was.

use super::*;

/// The payload with the value of its one `key` string swapped, every other byte as it was.
/// None unless the result parses to exactly the original with that one value changed.
pub(super) fn rewrite(raw: &str, key: &str, old: &str, new: &str) -> Option<String> {
    replace(
        raw,
        key,
        old,
        &format!("\"{new}\""),
        Value::String(new.to_owned()),
    )
}

/// `raw` with string `key`'s value `old` made null, written as [`rewrite`] writes.
pub(super) fn clear(raw: &str, key: &str, old: &str) -> Option<String> {
    replace(raw, key, old, "null", Value::Null)
}

/// `raw` with the one string `key` holding `old` written as `literal` (whose value is
/// `value`), the rest of the text as it was; None unless that's exactly what changed.
pub(super) fn replace(
    raw: &str,
    key: &str,
    old: &str,
    literal: &str,
    value: Value,
) -> Option<String> {
    let needle = format!("\"{key}\"");
    let mut found = raw.match_indices(&needle);
    let (at, _) = found.next()?;
    if found.next().is_some() {
        return None;
    }
    let after_key = at + needle.len();
    let rest = raw[after_key..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let value_at = raw.len() - rest.len();
    let (original, replacement) = (format!("\"{old}\""), literal);
    if !rest.starts_with(&original) {
        return None;
    }
    let rewritten = format!(
        "{}{replacement}{}",
        &raw[..value_at],
        &raw[value_at + original.len()..]
    );
    let mut expected: Value = serde_json::from_str(raw).ok()?;
    expected[key] = value;
    let actual: Value = serde_json::from_str(&rewritten).ok()?;
    (actual == expected).then_some(rewritten)
}
