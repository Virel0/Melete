//! A random order that pages: the same seed shuffles the same ids the same way, so the next
//! page of a shuffle, asked for with its seed, follows on from the last without repeating it.

/// Shuffles `ids` (sorted first, so the order depends only on which ids there are and the
/// seed) by Fisher–Yates, drawing from SplitMix64 started at `seed`.
pub(in super::super) fn shuffle(ids: &mut [i64], seed: u32) {
    ids.sort_unstable();
    let mut state = u64::from(seed);
    let mut next = || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    for i in (1..ids.len()).rev() {
        let bound = u64::try_from(i + 1).unwrap_or(u64::MAX);
        let j = usize::try_from(next() % bound).unwrap_or(0);
        ids.swap(i, j);
    }
}

/// The seed asked for, or a new one when none was.
pub(in super::super) fn seed_or_new(seed: Option<u32>) -> u32 {
    seed.unwrap_or_else(|| {
        let mut bytes = [0u8; 4];
        // Without the system's randomness the clock will do: it only orders a list.
        if getrandom::fill(&mut bytes).is_err() {
            bytes = crate::db::now_ms().to_le_bytes()[..4]
                .try_into()
                .unwrap_or_default();
        }
        u32::from_le_bytes(bytes)
    })
}

/// `ids` shuffled by `seed`: how many there are, and the page from `offset`, at most `limit`.
pub(in super::super) fn shuffled_page(
    mut ids: Vec<i64>,
    seed: u32,
    limit: i64,
    offset: i64,
) -> (usize, Vec<i64>) {
    shuffle(&mut ids, seed);
    let total = ids.len();
    let start = usize::try_from(offset).unwrap_or(usize::MAX).min(total);
    let end = start
        .saturating_add(usize::try_from(limit).unwrap_or(0))
        .min(total);
    (total, ids[start..end].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seed_shuffles_the_same_ids_the_same_way_and_pages_cover_them_once() {
        let ids: Vec<i64> = (1..=50).collect();
        let mut a = ids.clone();
        let mut b: Vec<i64> = ids.iter().rev().copied().collect();
        shuffle(&mut a, 7);
        shuffle(&mut b, 7);
        assert_eq!(a, b, "the order given doesn't matter");
        assert_ne!(a, ids, "it is shuffled");
        let mut c = ids.clone();
        shuffle(&mut c, 8);
        assert_ne!(a, c, "another seed, another order");
        let mut seen = Vec::new();
        for offset in (0..50).step_by(20) {
            let (total, page) = shuffled_page(ids.clone(), 7, 20, offset);
            assert_eq!(total, 50);
            seen.extend(page);
        }
        assert_eq!(seen, a, "the pages, in turn, are the one shuffle");
        assert_eq!(shuffled_page(ids, 7, 20, 60).1, Vec::<i64>::new());
    }
}
