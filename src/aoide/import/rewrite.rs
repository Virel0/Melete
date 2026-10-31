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
    let mut expected: Value = serde_json::from_str(raw).ok()?;
    if named_below(&expected, key) {
        return None;
    }
    let rest = &raw[top_level_value(raw, key)?..];
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
    expected[key] = value;
    let actual: Value = serde_json::from_str(&rewritten).ok()?;
    (actual == expected).then_some(rewritten)
}

/// Whether `key` names a field anywhere below the top level, apart from the per-field timestamps
/// in `fieldUpdatedAt`, which name every field they date: another copy would leave which one is
/// meant in doubt.
fn named_below(payload: &Value, key: &str) -> bool {
    fn anywhere(value: &Value, key: &str) -> bool {
        match value {
            Value::Object(fields) => fields.iter().any(|(k, v)| k == key || anywhere(v, key)),
            Value::Array(items) => items.iter().any(|v| anywhere(v, key)),
            _ => false,
        }
    }
    payload.as_object().is_some_and(|fields| {
        fields
            .iter()
            .any(|(k, v)| k != "fieldUpdatedAt" && anywhere(v, key))
    })
}

/// Where the value of the object's own `key` starts in `raw`: the key at the top level only, so
/// `fieldUpdatedAt`'s timestamp for it isn't taken for it.
/// None when the top level holds the key other than once, or `raw` isn't an object.
fn top_level_value(raw: &str, key: &str) -> Option<usize> {
    let bytes = raw.as_bytes();
    let (mut depth, mut i, mut found) = (0usize, 0usize, None);
    while i < bytes.len() {
        match bytes[i] {
            b'{' | b'[' => depth += 1,
            b'}' | b']' => depth = depth.checked_sub(1)?,
            b'"' => {
                let end = string_end(bytes, i)?;
                let rest = raw[end + 1..].trim_start();
                if depth == 1 && rest.starts_with(':') {
                    let name: String = serde_json::from_str(&raw[i..=end]).ok()?;
                    if name == key {
                        if found.is_some() {
                            return None;
                        }
                        found = Some(raw.len() - rest[1..].trim_start().len());
                    }
                }
                i = end;
            }
            _ => {}
        }
        i += 1;
    }
    found
}

/// The index of the quote that closes the string opening at `start`.
fn string_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_top_level_key_is_rewritten() {
        let raw = r#"{"id":"pl1","sourceJellyfinId":"b2b2","fieldUpdatedAt":{"sourceJellyfinId":1700000000},"n":"\"sourceJellyfinId\""}"#;
        assert_eq!(
            clear(raw, "sourceJellyfinId", "b2b2").as_deref(),
            Some(
                r#"{"id":"pl1","sourceJellyfinId":null,"fieldUpdatedAt":{"sourceJellyfinId":1700000000},"n":"\"sourceJellyfinId\""}"#
            )
        );
        let track = r#"{"id":"i1","fieldUpdatedAt":{"jellyfinId":5},"jellyfinId" : "1111","contentKey":"a"}"#;
        assert_eq!(
            rewrite(track, "jellyfinId", "1111", "track:9").as_deref(),
            Some(
                r#"{"id":"i1","fieldUpdatedAt":{"jellyfinId":5},"jellyfinId" : "track:9","contentKey":"a"}"#
            )
        );
    }

    #[test]
    fn a_key_held_twice_at_the_top_or_named_elsewhere_below_is_left_alone() {
        assert_eq!(clear(r#"{"k":"a","k":"a"}"#, "k", "a"), None);
        assert_eq!(clear(r#"{"x":{"k":"a"}}"#, "k", "a"), None);
        assert_eq!(clear(r#"{"k":"a","x":[{"k":1}]}"#, "k", "a"), None);
        assert_eq!(top_level_value("[1]", "k"), None);
    }
}
