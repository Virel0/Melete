//! Lyrics' pronunciation in Latin letters, worked out on the server, off unless
//! `MELETE_LYRICS_ROMANISATION=on`: Japanese (kana from a table, kanji read in context with
//! Lindera and its built-in IPADIC dictionary) in Hepburn, Korean hangul in the Revised
//! Romanization (syllable by syllable, without the sound changes between them), and Cyrillic
//! letter by letter. Nothing leaves the server.
//!
//! Chinese characters are read as Japanese only in lyrics that have kana somewhere: a song in
//! Chinese alone gets no pronunciation, since Japanese readings would be wrong for it.

use std::borrow::Cow;
use std::sync::OnceLock;

use lindera::dictionary::load_dictionary;
use lindera::mode::Mode;
use lindera::segmenter::Segmenter;

fn is_kana(c: char) -> bool {
    matches!(c, '\u{3041}'..='\u{309F}' | '\u{30A0}'..='\u{30FF}' | '\u{31F0}'..='\u{31FF}' | '\u{FF66}'..='\u{FF9F}')
}

fn is_kanji(c: char) -> bool {
    matches!(c, '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '\u{F900}'..='\u{FAFF}' | '々')
}

fn is_hangul(c: char) -> bool {
    matches!(c, '\u{AC00}'..='\u{D7A3}')
}

fn is_cyrillic(c: char) -> bool {
    matches!(c, '\u{0400}'..='\u{04FF}')
}

/// Whether lyrics are Japanese, for reading their Chinese characters: they have kana.
pub fn japanese(lyrics: &str) -> bool {
    lyrics.chars().any(is_kana)
}

/// One line's pronunciation; none when there's nothing in it to romanise.
pub fn line(text: &str, japanese: bool) -> Option<String> {
    let needed = text
        .chars()
        .any(|c| is_kana(c) || is_hangul(c) || is_cyrillic(c) || (japanese && is_kanji(c)));
    if !needed {
        return None;
    }
    let text = if japanese && text.chars().any(|c| is_kana(c) || is_kanji(c)) {
        Cow::Owned(japanese_line(text))
    } else {
        Cow::Borrowed(text)
    };
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if is_hangul(c) {
            out.push_str(&hangul(c));
        } else if is_cyrillic(c) {
            out.push_str(&cyrillic(c));
        } else if is_kana(c) {
            // Kana Japanese reading couldn't place (outside Japanese lyrics).
            out.push_str(&kana(&c.to_string()));
        } else {
            out.push(c);
        }
    }
    Some(out.split_whitespace().collect::<Vec<_>>().join(" "))
}

// ---------- Japanese ----------

fn segmenter() -> Option<&'static Segmenter> {
    static SEGMENTER: OnceLock<Option<Segmenter>> = OnceLock::new();
    SEGMENTER
        .get_or_init(|| match load_dictionary("embedded://ipadic") {
            Ok(dictionary) => Some(Segmenter::new(Mode::Normal, dictionary, None)),
            Err(err) => {
                tracing::error!(error = %err, "the Japanese dictionary couldn't be loaded");
                None
            }
        })
        .as_ref()
}

/// A Japanese line in romaji, word by word: each word's reading from the dictionary (the
/// particles は, へ and を as they're said), words it doesn't know as written.
fn japanese_line(text: &str) -> String {
    let Some(segmenter) = segmenter() else {
        return kana(text);
    };
    let Ok(mut tokens) = segmenter.segment(Cow::Borrowed(text)) else {
        return kana(text);
    };
    // Each word as kana to read, or as it's written.
    let mut words: Vec<(bool, String)> = Vec::new();
    for token in tokens.iter_mut() {
        let surface = token.surface.to_string();
        if !surface.chars().any(|c| is_kana(c) || is_kanji(c)) {
            words.push((false, surface));
            continue;
        }
        let details = token.details();
        let particle = details.first().is_some_and(|d| *d == "助詞");
        let word = match (surface.as_str(), particle) {
            ("は", true) => (false, "wa".to_owned()),
            ("へ", true) => (false, "e".to_owned()),
            ("を", true) => (false, "o".to_owned()),
            _ => match details.get(7).filter(|r| !r.is_empty() && **r != "*") {
                Some(reading) => (true, (*reading).to_owned()),
                // Not in the dictionary: kana as written, kanji left as they are.
                None => (true, surface.clone()),
            },
        };
        // A word ending in a small tsu doubles the next word's first consonant ("mukatte"),
        // and a long mark or small tsu alone belongs to the word before.
        if let Some((true, last)) = words.last_mut()
            && word.0
            && (last.ends_with(['ッ', 'っ'])
                || word.1.chars().all(|c| matches!(c, 'ー' | 'ッ' | 'っ')))
        {
            last.push_str(&word.1);
            continue;
        }
        words.push(word);
    }
    words
        .into_iter()
        .map(|(read, word)| if read { kana(&word) } else { word })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Kana, hiragana or katakana, in Hepburn: a small tsu doubles the next consonant, a long
/// mark doubles the vowel before, ん before a vowel or y is n'. Anything else is kept.
pub fn kana(text: &str) -> String {
    let chars: Vec<char> = text.chars().map(to_hiragana).collect();
    let mut out = String::new();
    let mut double = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // Two-kana syllables first: きゃ, しゅ, ちょ, ふぁ…
        let pair: Option<&str> = chars
            .get(i + 1)
            .and_then(|&next| YOON.iter().find(|(k, _)| *k == [c, next]))
            .map(|(_, r)| *r);
        let (romaji, used) = match pair {
            Some(r) => (r.to_owned(), 2),
            None => match c {
                'っ' => {
                    double = true;
                    i += 1;
                    continue;
                }
                'ー' => {
                    let vowel = out
                        .chars()
                        .rev()
                        .find(|v| "aeiou".contains(*v))
                        .map(String::from)
                        .unwrap_or_default();
                    (vowel, 1)
                }
                'ん' => {
                    let next = chars.get(i + 1).and_then(|&n| single(n));
                    let apostrophe =
                        next.is_some_and(|r| r.starts_with(['a', 'e', 'i', 'o', 'u', 'y']));
                    ((if apostrophe { "n'" } else { "n" }).to_owned(), 1)
                }
                _ => match single(c) {
                    Some(r) => (r.to_owned(), 1),
                    None => (c.to_string(), 1),
                },
            },
        };
        if double {
            double = false;
            match romaji.chars().next() {
                // ch doubles as tch.
                Some('c') => out.push('t'),
                Some(first) if first.is_ascii_alphabetic() && !"aeiou".contains(first) => {
                    out.push(first)
                }
                _ => {}
            }
        }
        out.push_str(&romaji);
        i += used;
    }
    out
}

fn to_hiragana(c: char) -> char {
    match c {
        // Katakana with hiragana twins; ー and the few without stay.
        '\u{30A1}'..='\u{30F6}' => char::from_u32(c as u32 - 0x60).unwrap_or(c),
        _ => c,
    }
}

fn single(c: char) -> Option<&'static str> {
    KANA.iter().find(|(k, _)| *k == c).map(|(_, r)| *r)
}

const KANA: &[(char, &str)] = &[
    ('あ', "a"),
    ('い', "i"),
    ('う', "u"),
    ('え', "e"),
    ('お', "o"),
    ('か', "ka"),
    ('き', "ki"),
    ('く', "ku"),
    ('け', "ke"),
    ('こ', "ko"),
    ('さ', "sa"),
    ('し', "shi"),
    ('す', "su"),
    ('せ', "se"),
    ('そ', "so"),
    ('た', "ta"),
    ('ち', "chi"),
    ('つ', "tsu"),
    ('て', "te"),
    ('と', "to"),
    ('な', "na"),
    ('に', "ni"),
    ('ぬ', "nu"),
    ('ね', "ne"),
    ('の', "no"),
    ('は', "ha"),
    ('ひ', "hi"),
    ('ふ', "fu"),
    ('へ', "he"),
    ('ほ', "ho"),
    ('ま', "ma"),
    ('み', "mi"),
    ('む', "mu"),
    ('め', "me"),
    ('も', "mo"),
    ('や', "ya"),
    ('ゆ', "yu"),
    ('よ', "yo"),
    ('ら', "ra"),
    ('り', "ri"),
    ('る', "ru"),
    ('れ', "re"),
    ('ろ', "ro"),
    ('わ', "wa"),
    ('ゐ', "i"),
    ('ゑ', "e"),
    ('を', "o"),
    ('が', "ga"),
    ('ぎ', "gi"),
    ('ぐ', "gu"),
    ('げ', "ge"),
    ('ご', "go"),
    ('ざ', "za"),
    ('じ', "ji"),
    ('ず', "zu"),
    ('ぜ', "ze"),
    ('ぞ', "zo"),
    ('だ', "da"),
    ('ぢ', "ji"),
    ('づ', "zu"),
    ('で', "de"),
    ('ど', "do"),
    ('ば', "ba"),
    ('び', "bi"),
    ('ぶ', "bu"),
    ('べ', "be"),
    ('ぼ', "bo"),
    ('ぱ', "pa"),
    ('ぴ', "pi"),
    ('ぷ', "pu"),
    ('ぺ', "pe"),
    ('ぽ', "po"),
    ('ゔ', "vu"),
    ('ぁ', "a"),
    ('ぃ', "i"),
    ('ぅ', "u"),
    ('ぇ', "e"),
    ('ぉ', "o"),
    ('ゃ', "ya"),
    ('ゅ', "yu"),
    ('ょ', "yo"),
    ('ゎ', "wa"),
];

const YOON: &[([char; 2], &str)] = &[
    (['き', 'ゃ'], "kya"),
    (['き', 'ゅ'], "kyu"),
    (['き', 'ょ'], "kyo"),
    (['し', 'ゃ'], "sha"),
    (['し', 'ゅ'], "shu"),
    (['し', 'ょ'], "sho"),
    (['し', 'ぇ'], "she"),
    (['ち', 'ゃ'], "cha"),
    (['ち', 'ゅ'], "chu"),
    (['ち', 'ょ'], "cho"),
    (['ち', 'ぇ'], "che"),
    (['に', 'ゃ'], "nya"),
    (['に', 'ゅ'], "nyu"),
    (['に', 'ょ'], "nyo"),
    (['ひ', 'ゃ'], "hya"),
    (['ひ', 'ゅ'], "hyu"),
    (['ひ', 'ょ'], "hyo"),
    (['み', 'ゃ'], "mya"),
    (['み', 'ゅ'], "myu"),
    (['み', 'ょ'], "myo"),
    (['り', 'ゃ'], "rya"),
    (['り', 'ゅ'], "ryu"),
    (['り', 'ょ'], "ryo"),
    (['ぎ', 'ゃ'], "gya"),
    (['ぎ', 'ゅ'], "gyu"),
    (['ぎ', 'ょ'], "gyo"),
    (['じ', 'ゃ'], "ja"),
    (['じ', 'ゅ'], "ju"),
    (['じ', 'ょ'], "jo"),
    (['じ', 'ぇ'], "je"),
    (['び', 'ゃ'], "bya"),
    (['び', 'ゅ'], "byu"),
    (['び', 'ょ'], "byo"),
    (['ぴ', 'ゃ'], "pya"),
    (['ぴ', 'ゅ'], "pyu"),
    (['ぴ', 'ょ'], "pyo"),
    (['ふ', 'ぁ'], "fa"),
    (['ふ', 'ぃ'], "fi"),
    (['ふ', 'ぇ'], "fe"),
    (['ふ', 'ぉ'], "fo"),
    (['て', 'ぃ'], "ti"),
    (['で', 'ぃ'], "di"),
    (['と', 'ぅ'], "tu"),
    (['ど', 'ぅ'], "du"),
    (['う', 'ぃ'], "wi"),
    (['う', 'ぇ'], "we"),
    (['う', 'ぉ'], "wo"),
    (['ゔ', 'ぁ'], "va"),
    (['ゔ', 'ぃ'], "vi"),
];

// ---------- Korean ----------

const INITIAL: [&str; 19] = [
    "g", "kk", "n", "d", "tt", "r", "m", "b", "pp", "s", "ss", "", "j", "jj", "ch", "k", "t", "p",
    "h",
];
const MEDIAL: [&str; 21] = [
    "a", "ae", "ya", "yae", "eo", "e", "yeo", "ye", "o", "wa", "wae", "oe", "yo", "u", "wo", "we",
    "wi", "yu", "eu", "ui", "i",
];
const FINAL: [&str; 28] = [
    "", "k", "k", "k", "n", "n", "n", "t", "l", "k", "m", "l", "l", "l", "p", "l", "m", "p", "p",
    "t", "t", "ng", "t", "t", "k", "t", "p", "t",
];

/// A hangul syllable in the Revised Romanization, on its own.
fn hangul(c: char) -> String {
    let index = c as u32 - 0xAC00;
    let (initial, medial, last) = (index / 588, (index % 588) / 28, index % 28);
    format!(
        "{}{}{}",
        INITIAL[initial as usize], MEDIAL[medial as usize], FINAL[last as usize]
    )
}

// ---------- Cyrillic ----------

/// A Cyrillic letter in Latin letters (Russian's common transliteration, with Ukrainian,
/// Belarusian, Serbian, Macedonian and Bulgarian letters), its case kept.
fn cyrillic(c: char) -> String {
    let lower: String = c.to_lowercase().collect();
    let latin = match lower.as_str() {
        "а" => "a",
        "б" => "b",
        "в" => "v",
        "г" => "g",
        "д" => "d",
        "е" => "e",
        "ё" => "yo",
        "ж" => "zh",
        "з" => "z",
        "и" => "i",
        "й" => "y",
        "к" => "k",
        "л" => "l",
        "м" => "m",
        "н" => "n",
        "о" => "o",
        "п" => "p",
        "р" => "r",
        "с" => "s",
        "т" => "t",
        "у" => "u",
        "ф" => "f",
        "х" => "kh",
        "ц" => "ts",
        "ч" => "ch",
        "ш" => "sh",
        "щ" => "shch",
        "ъ" => "",
        "ы" => "y",
        "ь" => "",
        "э" => "e",
        "ю" => "yu",
        "я" => "ya",
        "і" => "i",
        "ї" => "yi",
        "є" => "ye",
        "ґ" => "g",
        "ў" => "w",
        "ђ" => "đ",
        "ј" => "j",
        "љ" => "lj",
        "њ" => "nj",
        "ћ" => "ć",
        "џ" => "dž",
        "ѓ" => "gj",
        "ќ" => "kj",
        "ѕ" => "dz",
        _ => return c.to_string(),
    };
    if c.is_uppercase() {
        let mut letters = latin.chars();
        match letters.next() {
            Some(first) => first.to_uppercase().chain(letters).collect(),
            None => String::new(),
        }
    } else {
        latin.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kana_follow_hepburn() {
        assert_eq!(kana("さよなら"), "sayonara");
        assert_eq!(kana("きって"), "kitte");
        assert_eq!(kana("まっちゃ"), "matcha");
        assert_eq!(kana("コーヒー"), "koohii");
        assert_eq!(kana("きんようび"), "kin'youbi");
        assert_eq!(kana("しんぶん"), "shinbun");
        assert_eq!(kana("ファン"), "fan");
        assert_eq!(kana("メロディー"), "merodii");
    }

    #[test]
    fn japanese_lines_are_read_word_by_word_in_context() {
        let line = |s: &str| super::line(s, japanese(s)).unwrap();
        assert_eq!(line("東京の夜は眠らない"), "toukyou no yoru wa nemura nai");
        assert_eq!(line("君の名は"), "kimi no na wa");
        assert_eq!(line("空に向かって走った"), "sora ni mukatte hashitta");
        assert_eq!(line("残酷な天使のテーゼ"), "zankoku na tenshi no teeze");
        assert_eq!(line("I love you 君が好き"), "I love you kimi ga suki");
    }

    #[test]
    fn korean_and_cyrillic_are_spelt_out() {
        assert_eq!(super::line("사랑해", false).unwrap(), "saranghae");
        assert_eq!(
            super::line("안녕 하세요", false).unwrap(),
            "annyeong haseyo"
        );
        assert_eq!(super::line("Катюша", false).unwrap(), "Katyusha");
        assert_eq!(super::line("Щедрик", false).unwrap(), "Shchedrik");
    }

    #[test]
    fn latin_lines_and_chinese_alone_have_none() {
        assert_eq!(line("Hello, world", false), None);
        assert_eq!(
            line("我爱你", japanese("我爱你")),
            None,
            "Chinese isn't read as Japanese"
        );
        assert!(
            line("愛", true).is_some(),
            "in Japanese lyrics, kanji alone are read"
        );
    }
}
