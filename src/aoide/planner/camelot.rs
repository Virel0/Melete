//! Camelot keys: reading one, and whether two sit well together.

/// A Camelot key as its number and whether it's minor.
pub(super) fn parse_key(key: &str) -> Option<(i64, bool)> {
    let letter = key.chars().last()?.to_ascii_uppercase();
    if key.len() < 2 || !matches!(letter, 'A' | 'B') {
        return None;
    }
    let number: i64 = key[..key.len() - 1].parse().ok()?;
    (1..=12)
        .contains(&number)
        .then_some((number, letter == 'A'))
}

/// The same key, one step either way round the wheel, or the same number in the other
/// letter. False when either is unknown. It prefers, never refuses.
pub fn compatible(a: Option<&str>, b: Option<&str>) -> bool {
    let (Some((n1, minor1)), Some((n2, minor2))) = (a.and_then(parse_key), b.and_then(parse_key))
    else {
        return false;
    };
    if n1 == n2 {
        return true;
    }
    let distance = (n1 - n2).abs();
    distance.min(12 - distance) == 1 && minor1 == minor2
}
