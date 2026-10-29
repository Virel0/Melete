//! The apps' sortable position keys, ported from them so a key written here is the key a
//! device would write: a playlist's order is these keys compared as plain strings, and putting
//! a track between two others writes one row, not every row below it.

/// Digits before upper case before lower case: ASCII order, so a plain comparison orders keys.
const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// A digit's value; none past the key's end or for a character outside the alphabet, as the
/// apps read it.
fn digit(key: Option<&str>, index: usize) -> Option<usize> {
    let byte = *key?.as_bytes().get(index)?;
    ALPHABET.iter().position(|c| *c == byte)
}

/// A key that sorts strictly between `lower` and `upper` (`None` is unbounded). `lower` must
/// sort before `upper`.
pub fn between(lower: Option<&str>, upper: Option<&str>) -> String {
    let Some(upper) = upper else {
        return increment(lower);
    };
    let base = ALPHABET.len();
    let mut result: Vec<u8> = Vec::new();
    let mut bounded_above = Some(upper);
    let mut index = 0;
    loop {
        let low = digit(lower, index).unwrap_or(0);
        let high = digit(bounded_above, index).unwrap_or(base);
        if high - low > 1 {
            // Never the smallest digit, so there's always room before a key.
            result.push(ALPHABET[(low + high) / 2]);
            return String::from_utf8_lossy(&result).into_owned();
        }
        result.push(ALPHABET[low]);
        // Below `upper` at this digit: every extension stays below it.
        if low < high {
            bounded_above = None;
        }
        index += 1;
    }
}

/// Keys for `count` items put between `lower` and `upper`, in order: counting upwards at the
/// end, else bisecting what's left of the gap.
pub fn positions(count: usize, lower: Option<&str>, upper: Option<&str>) -> Vec<String> {
    let mut keys = Vec::with_capacity(count);
    let mut previous = lower.map(str::to_owned);
    for _ in 0..count {
        let key = between(previous.as_deref(), upper);
        keys.push(key.clone());
        previous = Some(key);
    }
    keys
}

/// The next key after `key`, and the shortest one there is: appending counts.
fn increment(key: Option<&str>) -> String {
    let middle = ALPHABET[ALPHABET.len() / 2] as char;
    let Some(key) = key.filter(|k| !k.is_empty()) else {
        return middle.to_string();
    };
    let mut characters: Vec<u8> = key.bytes().collect();
    let mut index = characters.len();
    while index > 0 {
        index -= 1;
        let value = ALPHABET
            .iter()
            .position(|c| *c == characters[index])
            .unwrap_or(0);
        if value + 1 < ALPHABET.len() {
            characters[index] = ALPHABET[value + 1];
            characters.truncate(index + 1);
            return String::from_utf8_lossy(&characters).into_owned();
        }
    }
    format!("{key}{middle}")
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn the_first_keys_are_the_apps() {
        assert_eq!(between(None, None), "V");
        assert_eq!(positions(3, None, None), ["V", "W", "X"]);
        assert_eq!(between(Some("V"), Some("W")), "VV");
        assert_eq!(between(None, Some("V")), "F");
        assert_eq!(between(Some("Vz"), None), "W");
    }

    #[test]
    fn a_key_lands_strictly_between_and_there_is_always_room_before() {
        let mut low = between(None, None);
        let high = between(Some(&low), None);
        for step in 0..200 {
            let mid = between(Some(&low), Some(&high));
            assert!(low < mid && mid < high, "step {step}: {low} {mid} {high}");
            low = mid;
        }
        let mut first = between(None, None);
        for _ in 0..200 {
            let earlier = between(None, Some(&first));
            assert!(earlier < first && !earlier.ends_with('0'), "{earlier}");
            first = earlier;
        }
    }

    #[test]
    fn a_list_stays_sorted_and_short_under_insertion() {
        let mut keys = positions(10, None, None);
        for step in 0..100 {
            let at = (step * 7) % (keys.len() + 1);
            let lower = at.checked_sub(1).map(|i| keys[i].clone());
            let key = between(lower.as_deref(), keys.get(at).map(String::as_str));
            keys.insert(at, key);
            let mut sorted = keys.clone();
            sorted.sort();
            assert_eq!(keys, sorted, "step {step}");
        }
        assert_eq!(keys.iter().collect::<HashSet<_>>().len(), keys.len());
        let longest = positions(1000, None, None).iter().map(String::len).max();
        assert!(longest.unwrap_or(0) <= 40);
        // Between an old list's keys, too.
        assert!(("a0".to_owned()..="a1".to_owned()).contains(&between(Some("a0"), Some("a1"))));
    }
}
