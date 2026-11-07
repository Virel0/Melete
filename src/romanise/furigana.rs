//! Readings over kanji in Japanese lyrics (furigana), for drawing with ruby: each word read
//! in context with the dictionary the pronunciation uses, its reading in hiragana over its
//! kanji only. The kana written in a word stay as they are (the ら of 眠ら), and so do kana,
//! Latin letters and everything else between words. Nothing leaves the server.

use std::borrow::Cow;

use serde::Serialize;

use super::{is_kana, is_kanji, segmenter, to_hiragana};

/// A piece of a line: its text as written, and over kanji, how they're read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Piece {
    pub text: String,
    /// In hiragana; none for anything but kanji, and for kanji the dictionary doesn't know.
    pub reading: Option<String>,
}

/// A line in pieces that join up into it again, its kanji with their readings; none when the
/// line has no kanji, or the lyrics aren't Japanese (no kana anywhere in them), as for its
/// pronunciation.
pub fn ruby(text: &str, japanese: bool) -> Option<Vec<Piece>> {
    if !japanese || !text.chars().any(is_kanji) {
        return None;
    }
    let mut tokens = segmenter()?.segment(Cow::Borrowed(text)).ok()?;
    let mut pieces = Vec::new();
    let mut done = 0;
    for token in tokens.iter_mut() {
        let (start, end) = (token.byte_start, token.byte_end);
        if start < done
            || end > text.len()
            || !text.is_char_boundary(start)
            || !text.is_char_boundary(end)
        {
            continue;
        }
        // Whatever the segmenter passed over (spaces, say) stays as it is.
        push(&mut pieces, &text[done..start], None);
        done = end;
        let surface = &text[start..end];
        if !surface.chars().any(is_kanji) {
            push(&mut pieces, surface, None);
            continue;
        }
        let reading = token
            .details()
            .get(7)
            .filter(|r| !r.is_empty() && **r != "*")
            .map(|r| r.chars().map(to_hiragana).collect::<String>());
        match reading {
            Some(reading) => {
                for (text, reading) in over_kanji(surface, &reading) {
                    push(&mut pieces, &text, reading);
                }
            }
            None => push(&mut pieces, surface, None),
        }
    }
    push(&mut pieces, &text[done..], None);
    Some(pieces)
}

/// Adds a piece, joining text without a reading to the piece before when it has none either.
fn push(pieces: &mut Vec<Piece>, text: &str, reading: Option<String>) {
    if text.is_empty() {
        return;
    }
    if reading.is_none()
        && let Some(last) = pieces.last_mut()
        && last.reading.is_none()
    {
        last.text.push_str(text);
        return;
    }
    pieces.push(Piece {
        text: text.to_owned(),
        reading,
    });
}

/// A word's reading put over its kanji only: the kana written in it are found in its reading,
/// and what's left between them is the kanji's. The whole reading over the whole word when the
/// two can't be matched up so.
fn over_kanji(word: &str, reading: &str) -> Vec<(String, Option<String>)> {
    // The word in runs of kanji and of the rest.
    let mut runs: Vec<(bool, String)> = Vec::new();
    for c in word.chars() {
        let kanji = is_kanji(c);
        match runs.last_mut() {
            Some((k, run)) if *k == kanji => run.push(c),
            _ => runs.push((kanji, c.to_string())),
        }
    }
    let reading: Vec<char> = reading.chars().collect();
    match matched(&runs, &reading) {
        Some(readings) => runs
            .into_iter()
            .zip(readings)
            .map(|((_, text), reading)| (text, reading))
            .collect(),
        None => vec![(word.to_owned(), Some(reading.iter().collect()))],
    }
}

/// Each run's reading (none for a run of kana), when the runs read as `reading`: a run of kana
/// as written (katakana as hiragana), a run of kanji as one kana or more.
fn matched(runs: &[(bool, String)], reading: &[char]) -> Option<Vec<Option<String>>> {
    let Some(((kanji, run), rest)) = runs.split_first() else {
        return reading.is_empty().then(Vec::new);
    };
    if !kanji {
        let written: Vec<char> = run.chars().map(to_hiragana).collect();
        if !run.chars().all(is_kana) || !reading.starts_with(&written) {
            return None;
        }
        let mut readings = matched(rest, &reading[written.len()..])?;
        readings.insert(0, None);
        return Some(readings);
    }
    // The fewest kana that let the rest match, so a kana after the kanji is the word's own.
    (1..=reading.len()).find_map(|taken| {
        let mut readings = matched(rest, &reading[taken..])?;
        readings.insert(0, Some(reading[..taken].iter().collect()));
        Some(readings)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line's pieces as "kanji(reading)" and plain text, joined by |.
    fn shown(line: &str) -> String {
        let pieces = ruby(line, crate::romanise::japanese(line)).unwrap();
        let joined: String = pieces.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(joined, line, "the pieces join up into the line");
        pieces
            .iter()
            .map(|p| match &p.reading {
                Some(r) => format!("{}({r})", p.text),
                None => p.text.clone(),
            })
            .collect::<Vec<_>>()
            .join("|")
    }

    #[test]
    fn kanji_get_their_readings_and_kana_stay_as_written() {
        assert_eq!(
            shown("東京の夜は眠らない"),
            "東京(とうきょう)|の|夜(よる)|は|眠(ねむ)|らない"
        );
        assert_eq!(
            shown("空に向かって走った"),
            "空(そら)|に|向(む)|かって|走(はし)|った"
        );
        assert_eq!(shown("君の名は"), "君(きみ)|の|名(な)|は");
        assert_eq!(
            shown("残酷な天使のテーゼ"),
            "残酷(ざんこく)|な|天使(てんし)|のテーゼ"
        );
    }

    #[test]
    fn latin_letters_and_spaces_are_left_alone() {
        assert_eq!(
            shown("I love you 君が好き"),
            "I love you |君(きみ)|が|好(す)|き"
        );
        assert_eq!(shown("Hello 世界だよ"), "Hello |世界(せかい)|だよ");
    }

    #[test]
    fn lines_without_kanji_and_chinese_alone_have_none() {
        for line in ["さよなら、またね", "メロディー", "Hello, world", ""] {
            assert_eq!(ruby(line, true), None, "{line}");
        }
        assert_eq!(ruby("我爱你", crate::romanise::japanese("我爱你")), None);
    }

    #[test]
    fn a_reading_that_cant_be_matched_goes_over_the_whole_word() {
        assert_eq!(
            over_kanji("1人", "ひとり"),
            vec![("1人".to_owned(), Some("ひとり".to_owned()))]
        );
        assert_eq!(
            over_kanji("引き出し", "ひきだし"),
            vec![
                ("引".to_owned(), Some("ひ".to_owned())),
                ("き".to_owned(), None),
                ("出".to_owned(), Some("だ".to_owned())),
                ("し".to_owned(), None),
            ]
        );
    }
}
