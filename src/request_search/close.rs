//! How close a search result is to the words typed, as one number from 0 to 100, so results
//! from different sources (MusicBrainz's songs and Soulseek's) can be ranked in one list. Words
//! are compared folded, as searches compare them: "spider man" is "Spider-Man", "amelie" is
//! "Amélie".

use std::collections::BTreeSet;

use crate::search_keys::fold;

/// Points for the typed words found among the title's and artist's words.
const FOUND: usize = 60;
/// Points for the title's words that were typed.
const COVERED: usize = 40;

/// A name's words, each folded; what separates them as well as spaces: "Spider-Man" and
/// "AC/DC" are two words each.
fn words(text: &str) -> BTreeSet<String> {
    text.split(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '-' | '_' | '/' | '\\' | '(' | ')' | '[' | ']' | ',' | ':' | ';' | '+'
            )
    })
    .map(fold)
    .filter(|w| !w.is_empty())
    .collect()
}

/// How close `title` by `artist` is to `q`, from 0 (nothing typed is in it) to 100 (it is what
/// was typed). Each typed word counts in full when it's one of the title's or the artist's
/// words and half when it starts one; that share is worth 60. The share of the title's words
/// that were typed is worth 40, so a title with words nobody typed ("Creep (Live)") comes after
/// the bare one ("Creep"). A title typed whole, alone or with its artist before or after it,
/// spaces or not, is 100.
pub fn how_close(q: &str, title: &str, artist: Option<&str>) -> u8 {
    let typed = words(q);
    if typed.is_empty() {
        return 0;
    }
    let artist = artist.unwrap_or_default();
    let all_typed = fold(q);
    let whole = [
        fold(title),
        fold(&format!("{artist}{title}")),
        fold(&format!("{title}{artist}")),
    ];
    if whole.contains(&all_typed) {
        return 100;
    }
    let named = words(title);
    let mut named_or_by = named.clone();
    named_or_by.extend(words(artist));
    let found: usize = typed
        .iter()
        .map(|w| {
            if named_or_by.contains(w) {
                2
            } else if named_or_by.iter().any(|n| n.starts_with(w.as_str())) {
                1
            } else {
                0
            }
        })
        .sum();
    let covered = named.iter().filter(|n| typed.contains(*n)).count();
    let found_points = FOUND * found / (2 * typed.len());
    let covered_points = COVERED * covered / named.len().max(1);
    u8::try_from(found_points + covered_points).unwrap_or(100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_was_typed_whole_is_closest() {
        assert_eq!(how_close("Creep", "Creep", Some("Radiohead")), 100);
        assert_eq!(
            how_close("radiohead creep", "Creep", Some("Radiohead")),
            100
        );
        assert_eq!(
            how_close("creep radiohead", "Creep", Some("Radiohead")),
            100
        );
        assert_eq!(how_close("spiderman", "Spider-Man", None), 100);
        assert_eq!(how_close("amelie", "Amélie", None), 100);
    }

    #[test]
    fn typed_words_found_count_most_then_the_title_typed() {
        // Every word found, the whole title typed, in another order.
        assert_eq!(
            how_close("android paranoid", "Paranoid Android", Some("Radiohead")),
            100
        );
        // Every word found; half the title typed.
        assert_eq!(how_close("creep", "Creep (Live)", Some("Radiohead")), 80);
        // Half the words found; the whole title typed.
        assert_eq!(how_close("radiohead creep", "Creep", Some("TLC")), 70);
        // Only the artist typed.
        assert_eq!(how_close("radiohead", "Creep", Some("Radiohead")), 60);
        // A word that only starts one of the title's counts half.
        assert_eq!(how_close("creep", "Creeping Death", Some("Metallica")), 30);
        assert_eq!(
            how_close("garage song", "Paranoid Android", Some("Radiohead")),
            0
        );
        assert_eq!(how_close("  ", "Creep", Some("Radiohead")), 0);
    }

    #[test]
    fn closer_results_rank_first_whatever_their_source() {
        let q = "radiohead creep";
        let mut found = [
            ("Creep (Acoustic)", "Radiohead"),
            ("Creep", "TLC"),
            ("Paranoid Android", "Radiohead"),
            ("Creep", "Radiohead"),
        ];
        found.sort_by_key(|(title, artist)| std::cmp::Reverse(how_close(q, title, Some(artist))));
        assert_eq!(
            found,
            [
                ("Creep", "Radiohead"),
                ("Creep (Acoustic)", "Radiohead"),
                ("Creep", "TLC"),
                ("Paranoid Android", "Radiohead"),
            ]
        );
    }
}
