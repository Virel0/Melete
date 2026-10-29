//! What a player needs to join tracks without a gap: how many samples at the start are
//! the encoder's priming (encoder delay) and how many at the end are padding. Lossless formats
//! have neither. Lossy files say so in format-specific places, read here from the file's bytes:
//! an MP3's LAME header, an MP4's iTunSMPB tag, an Opus stream's pre-skip. Nothing is written.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Samples to drop at the start and, when known, at the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gapless {
    /// Samples of the encoder's priming at the start (with an MP3 decoder's own delay added); 0 for
    /// lossless.
    pub encoder_delay: i64,
    /// Samples of padding at the end; none when the file doesn't say.
    pub padding: Option<i64>,
}

/// An MP3 decoder's own delay, added to the encoder's (the LAME convention).
const MP3_DECODER_DELAY: i64 = 529;
/// How much of an MP4's start and end is searched for its iTunSMPB tag (it's in the moov box,
/// which sits at one end or the other).
const MP4_SEARCH: u64 = 2 * 1024 * 1024;

/// Reads it for a file of the given codec (as `tags::Tags::codec` names them); `None` when the
/// file doesn't say.
pub fn read(path: &Path, codec: Option<&str>) -> Option<Gapless> {
    match codec? {
        "flac" | "alac" | "wav" | "aiff" | "wavpack" | "ape" => Some(Gapless {
            encoder_delay: 0,
            padding: Some(0),
        }),
        "mp3" => mp3(&head(path, 64 * 1024)?),
        "aac" => mp4(path),
        "opus" => opus(&head(path, 64 * 1024)?),
        _ => None,
    }
}

fn head(path: &Path, bytes: u64) -> Option<Vec<u8>> {
    let mut buffer = Vec::new();
    File::open(path)
        .ok()?
        .take(bytes)
        .read_to_end(&mut buffer)
        .ok()?;
    Some(buffer)
}

/// The LAME header's delay and padding (12 bits each), in the Xing or Info frame that starts
/// the stream, plus the decoder's delay.
pub fn mp3(data: &[u8]) -> Option<Gapless> {
    // Skip an ID3v2 tag: "ID3", version, flags, then a 28-bit size in four 7-bit bytes.
    let mut start = 0;
    if data.len() > 10 && &data[..3] == b"ID3" {
        let size = data[6..10]
            .iter()
            .fold(0usize, |n, b| (n << 7) | usize::from(b & 0x7f));
        start = 10 + size;
    }
    let frame = (start..data.len().saturating_sub(4))
        .find(|&i| data[i] == 0xff && data[i + 1] & 0xe0 == 0xe0)?;
    let header = &data[frame..];
    let mpeg1 = header[1] & 0x18 == 0x18;
    let mono = header[3] & 0xc0 == 0xc0;
    let side_info = match (mpeg1, mono) {
        (true, false) => 32,
        (true, true) | (false, false) => 17,
        (false, true) => 9,
    };
    let xing = frame + 4 + side_info;
    let tag = data.get(xing..xing + 8)?;
    if &tag[..4] != b"Xing" && &tag[..4] != b"Info" {
        return None;
    }
    let flags = u32::from_be_bytes(tag[4..8].try_into().ok()?);
    // The fields present (frames, bytes, table of contents, quality) set where LAME's starts.
    let lame = xing
        + 8
        + if flags & 1 != 0 { 4 } else { 0 }
        + if flags & 2 != 0 { 4 } else { 0 }
        + if flags & 4 != 0 { 100 } else { 0 }
        + if flags & 8 != 0 { 4 } else { 0 };
    let fields = data.get(lame + 21..lame + 24)?;
    let delay = (i64::from(fields[0]) << 4) | (i64::from(fields[1]) >> 4);
    let padding = (i64::from(fields[1] & 0x0f) << 8) | i64::from(fields[2]);
    Some(Gapless {
        encoder_delay: delay + MP3_DECODER_DELAY,
        padding: Some((padding - MP3_DECODER_DELAY).max(0)),
    })
}

/// iTunes' "iTunSMPB": " 00000000 00000840 000001CA 0000000000A5C5F6 …", hex fields of which
/// the second is the delay and the third the padding.
pub fn mp4(path: &Path) -> Option<Gapless> {
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let mut chunks = vec![head(path, MP4_SEARCH)?];
    if length > MP4_SEARCH {
        let mut tail = Vec::new();
        file.seek(SeekFrom::Start(length - MP4_SEARCH)).ok()?;
        file.take(MP4_SEARCH).read_to_end(&mut tail).ok()?;
        chunks.push(tail);
    }
    chunks.iter().find_map(|chunk| itunsmpb(chunk))
}

fn itunsmpb(data: &[u8]) -> Option<Gapless> {
    let at = data.windows(8).position(|w| w == b"iTunSMPB")?;
    // The value follows in a "data" atom: 4 bytes of type and 4 of locale, then the text.
    let after = &data[at + 8..];
    let value = after.windows(4).position(|w| w == b"data")? + 12;
    let text: String = after
        .get(value..value.checked_add(120)?.min(after.len()))?
        .iter()
        .take_while(|b| b.is_ascii_graphic() || **b == b' ')
        .map(|b| char::from(*b))
        .collect();
    let fields: Vec<i64> = text
        .split_whitespace()
        .filter_map(|f| i64::from_str_radix(f, 16).ok())
        .collect();
    match fields[..] {
        [_, delay, padding, ..] => Some(Gapless {
            encoder_delay: delay,
            padding: Some(padding),
        }),
        _ => None,
    }
}

/// An Opus stream's pre-skip, little-endian at byte 10 of its "OpusHead" packet. Its padding
/// is set by the last page's position, which players read themselves.
pub fn opus(data: &[u8]) -> Option<Gapless> {
    let at = data.windows(8).position(|w| w == b"OpusHead")?;
    let skip = data.get(at + 10..at + 12)?;
    Some(Gapless {
        encoder_delay: i64::from(u16::from_le_bytes([skip[0], skip[1]])),
        padding: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tone encoded by ffmpeg into `name`, returned with its path.
    fn encode(dir: &Path, name: &str, args: &[&str]) -> std::path::PathBuf {
        let path = dir.join(name);
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=d=1:r=44100",
            ])
            .args(args)
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success(), "ffmpeg made {name}");
        path
    }

    #[test]
    fn reads_an_mp3s_lame_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = encode(dir.path(), "t.mp3", &["-c:a", "libmp3lame", "-b:a", "128k"]);
        let found = read(&path, Some("mp3")).unwrap();
        // LAME primes 576 samples; with the decoder's 529, a player drops 1105.
        assert_eq!(found.encoder_delay, 576 + MP3_DECODER_DELAY);
        // One second at 44.1 kHz in 1152-sample frames: what's beyond it, less the decoder's.
        let padding = found.padding.unwrap();
        assert!((0..1152).contains(&padding), "{padding}");
        // With an ID3 tag in front it's still found.
        let tagged = encode(
            dir.path(),
            "tagged.mp3",
            &["-c:a", "libmp3lame", "-metadata", "title=x"],
        );
        assert_eq!(
            read(&tagged, Some("mp3")).unwrap().encoder_delay,
            found.encoder_delay
        );
    }

    #[test]
    fn reads_opus_pre_skip_and_knows_lossless_has_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = encode(dir.path(), "t.opus", &["-c:a", "libopus"]);
        assert_eq!(
            read(&path, Some("opus")),
            Some(Gapless {
                encoder_delay: 312,
                padding: None
            })
        );
        let flac = encode(dir.path(), "t.flac", &[]);
        assert_eq!(
            read(&flac, Some("flac")),
            Some(Gapless {
                encoder_delay: 0,
                padding: Some(0)
            })
        );
        assert_eq!(read(&flac, Some("vorbis")), None);
        assert_eq!(read(&flac, None), None);
    }

    #[test]
    fn reads_itunsmpb() {
        let mut atom =
            b"....----....meancom.apple.iTunes....nameiTunSMPB....data\0\0\0\x01\0\0\0\0".to_vec();
        atom.extend_from_slice(b" 00000000 00000840 000001CA 0000000000A5C5F6 00000000 00000000\0");
        assert_eq!(
            itunsmpb(&atom),
            Some(Gapless {
                encoder_delay: 0x840,
                padding: Some(0x1ca)
            })
        );
        assert_eq!(itunsmpb(b"no such tag"), None);
        // An MP4 without one (ffmpeg's AAC writes an edit list instead) says nothing.
        let dir = tempfile::tempdir().unwrap();
        let path = encode(dir.path(), "t.m4a", &["-c:a", "aac"]);
        assert_eq!(read(&path, Some("aac")), None);
    }

    #[test]
    fn junk_says_nothing() {
        assert_eq!(mp3(b"\xff\xfb\x90\x00 not a xing frame at all"), None);
        assert_eq!(mp3(b"ID3\x04\0\0\x7f\x7f\x7f\x7f"), None);
        assert_eq!(opus(b"OpusHead"), None);
    }
}
