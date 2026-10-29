//! Subtitle files that aren't UTF-8. ffmpeg's text subtitle decoders expect UTF-8 and drop or
//! mangle anything else, and older subtitle files are often in a legacy encoding (Windows-1251,
//! GBK, Shift_JIS…). Their encoding is guessed from their bytes, with the language in the file
//! name as a hint, and they're converted to UTF-8 before ffmpeg reads them.

use chardetng::{Iso2022JpDetection, Utf8Detection};
use encoding_rs::{Encoding, UTF_8};

use super::*;

/// The most of a subtitle file read; anything bigger isn't one.
pub const SUBTITLE_FILE_LIMIT: u64 = 20 * 1024 * 1024;

/// The encoding a subtitle file's text is in: the one its byte-order mark names, UTF-8 when
/// its bytes are valid UTF-8, else a guess (chardetng's, as Firefox makes for pages without
/// one), helped by the file name's language when there is one.
pub fn guess_encoding(bytes: &[u8], file_name: &str) -> &'static Encoding {
    if let Some((encoding, _)) = Encoding::for_bom(bytes) {
        return encoding;
    }
    // ISO-2022-JP is 7-bit, so valid UTF-8 too; its escapes give it away.
    let utf8 = std::str::from_utf8(bytes).is_ok();
    if utf8 && !bytes.contains(&0x1b) {
        return UTF_8;
    }
    let mut detector = chardetng::EncodingDetector::new(Iso2022JpDetection::Allow);
    detector.feed(bytes, true);
    let allow_utf8 = if utf8 {
        Utf8Detection::Allow
    } else {
        Utf8Detection::Deny
    };
    detector.guess(language_hint(file_name), allow_utf8)
}

/// The text of a subtitle file as UTF-8, and the encoding it was read as; None when it's
/// UTF-8 already and needs nothing done.
pub fn to_utf8(bytes: &[u8], file_name: &str) -> (&'static Encoding, Option<String>) {
    let encoding = guess_encoding(bytes, file_name);
    if encoding == UTF_8 {
        return (encoding, None);
    }
    let (text, _, _) = encoding.decode(bytes);
    (encoding, Some(text.into_owned()))
}

/// The encoding a subtitle file beside a video is read as, by its name ("UTF-8",
/// "windows-1251", "Shift_JIS"…), as converting it reads it; None when it can't be read or is
/// too big to be one.
pub fn file_encoding(path: &Path) -> Option<&'static str> {
    let bytes = read_limited(path).ok()?;
    let name = path.file_name()?.to_str()?;
    Some(guess_encoding(&bytes, name).name())
}

/// Reads a subtitle file, refusing one bigger than [`SUBTITLE_FILE_LIMIT`].
pub(super) fn read_limited(path: &Path) -> anyhow::Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(SUBTITLE_FILE_LIMIT + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > SUBTITLE_FILE_LIMIT {
        bail!("the subtitle file is over 20 MB, too big to be one");
    }
    Ok(bytes)
}

/// The top-level domain chardetng takes as a hint, for the language a subtitle file's name
/// gives ("Movie.ru.srt", "Movie.pol.forced.srt"): the tag nearest the extension that is a
/// language with a legacy encoding of its own. Chinese has none, as its name doesn't say
/// whether GBK or Big5 is likelier.
fn language_hint(file_name: &str) -> Option<&'static [u8]> {
    file_name.rsplit('.').skip(1).take(4).find_map(|tag| {
        let tag = tag.to_ascii_lowercase();
        HINTS
            .iter()
            .find(|(codes, _)| codes.contains(&tag.as_str()))
            .map(|(_, tld)| tld.as_bytes())
    })
}

/// Language tags (ISO 639-1 and both 639-2 forms) and the domain whose pages chardetng
/// expects in that language's legacy encodings.
const HINTS: &[(&[&str], &str)] = &[
    (&["pl", "pol"], "pl"),
    (&["cs", "cze", "ces"], "cz"),
    (&["sk", "slo", "slk"], "sk"),
    (&["hu", "hun"], "hu"),
    (&["sl", "slv"], "si"),
    (&["hr", "hrv"], "hr"),
    (&["ro", "rum", "ron"], "ro"),
    (&["bs", "bos"], "ba"),
    (&["sr", "srp"], "rs"),
    (&["ru", "rus"], "ru"),
    (&["uk", "ukr"], "ua"),
    (&["bg", "bul"], "bg"),
    (&["be", "bel"], "by"),
    (&["mk", "mac", "mkd"], "mk"),
    (&["el", "gre", "ell"], "gr"),
    (&["tr", "tur"], "tr"),
    (&["he", "heb"], "il"),
    (&["ar", "ara"], "sa"),
    (&["fa", "per", "fas"], "ir"),
    (&["th", "tha"], "th"),
    (&["vi", "vie"], "vn"),
    (&["ko", "kor"], "kr"),
    (&["ja", "jpn"], "jp"),
    (&["lt", "lit"], "lt"),
    (&["lv", "lav"], "lv"),
    (&["et", "est"], "ee"),
];

#[cfg(test)]
mod tests;
