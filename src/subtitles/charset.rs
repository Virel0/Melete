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

/// A subtitle file beside a video for players that read the file themselves, and the charset to
/// send it with.
pub struct Original {
    pub bytes: Vec<u8>,
    /// "utf-8", or the encoding the file was guessed to be in when it was kept as it is.
    pub charset: &'static str,
    /// Whether its text was converted from another encoding, so worth caching.
    pub converted: bool,
}

/// A subtitle file in UTF-8, since some players (libass built without iconv) read nothing else:
/// its bytes as they are when they're UTF-8 already, less a byte-order mark; else its text
/// converted from the encoding [`file_encoding`] gives, and nothing else changed.
pub fn original_file(path: &Path) -> anyhow::Result<Original> {
    let bytes = read_limited(path)?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    Ok(in_utf8(bytes, name))
}

/// The bytes of a subtitle file named `file_name` in UTF-8, as [`original_file`] gives them.
pub fn in_utf8(mut bytes: Vec<u8>, file_name: &str) -> Original {
    let encoding = guess_encoding(&bytes, file_name);
    if encoding == UTF_8 {
        if bytes.starts_with(UTF8_BOM) {
            bytes.drain(..UTF8_BOM.len());
        }
        return Original {
            bytes,
            charset: "utf-8",
            converted: false,
        };
    }
    match decoded(&bytes, encoding) {
        Some(text) => Original {
            bytes: text.into_bytes(),
            charset: "utf-8",
            converted: true,
        },
        // Unsure of the encoding: kept as it is, with the guess as its charset, as before.
        None => Original {
            bytes,
            charset: encoding.name(),
            converted: false,
        },
    }
}

const UTF8_BOM: &[u8] = b"\xef\xbb\xbf";

/// The text in `encoding`, less a byte-order mark; None when the guess is unsure. chardetng
/// gives no confidence of its own, so a guess whose decoding finds malformed bytes counts as
/// unsure: converting would put replacement characters in place of the text.
fn decoded(bytes: &[u8], encoding: &'static Encoding) -> Option<String> {
    let (text, had_errors) = encoding.decode_with_bom_removal(bytes);
    (!had_errors).then(|| text.into_owned())
}

/// A subtitle file beside a video in UTF-8, as [`original_file`] gives it, and the charset to
/// send it with. One that was converted is cached as `cache_dir/{key}.orig.ass`, `key` being
/// its [`sidecar_key`], so it's converted once and again only when the file changes.
pub async fn original_beside(
    cache_dir: &Path,
    key: &str,
    path: &Path,
) -> anyhow::Result<(Vec<u8>, &'static str)> {
    let cached = cache_dir.join(format!("{key}.orig.ass"));
    if let Ok(bytes) = tokio::fs::read(&cached).await {
        return Ok((bytes, "utf-8"));
    }
    let file = path.to_path_buf();
    let original = tokio::task::spawn_blocking(move || original_file(&file)).await??;
    if original.converted {
        // Not caching only costs converting it again next time.
        if let Err(err) = super::converting::save_atomically(&cached, &original.bytes).await {
            tracing::warn!(
                error = format!("{err:#}"),
                "caching a converted subtitle file failed"
            );
        }
    }
    Ok((original.bytes, original.charset))
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
