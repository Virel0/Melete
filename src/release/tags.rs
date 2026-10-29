//! What a release's tags say: resolution, source, codecs, audio channels, languages, version
//! and propers.

use super::*;

/// The resolution the tags name.
pub(super) fn resolution(has: &impl Fn(&Regex) -> bool) -> Option<i64> {
    [
        (&*R2160, 2160),
        (&*R1080, 1080),
        (&*R720, 720),
        (&*R576, 576),
        (&*R480, 480),
        (&*R4K, 2160),
    ]
    .into_iter()
    .find(|(r, _)| has(r))
    .map(|(_, res)| res)
}

/// Whether the tags name a remux, the source, and whether it's a full disc.
pub(super) fn source_and_disc(
    tags: &str,
    has: &impl Fn(&Regex) -> bool,
) -> (bool, Option<Source>, bool) {
    let dvd = has(&DVDRIP) || has(&DVD);
    let remux = has(&REMUX);
    // From the most to the least specific: "WEBRip" before "WEB", Blu-ray before DVD.
    let source = [
        (has(&CAMRIP) || has(&CAM), Source::Cam),
        (has(&TELESYNCS) || has(&TELESYNC), Source::Telesync),
        (has(&TELECINES) || has(&TELECINE), Source::Telecine),
        (has(&SCREENERS) || has(&SCREENER), Source::Screener),
        (has(&BLURAY) || has(&BD) || (remux && !dvd), Source::Bluray),
        (has(&WEBRIP) || has(&HDRIP), Source::WebRip),
        (has(&WEBDL) || has(&WEB), Source::WebDl),
        (has(&HDTV), Source::Hdtv),
        (dvd, Source::Dvd),
    ]
    .into_iter()
    .find(|(found, _)| *found)
    .map(|(_, source)| source);
    let encoded =
        has(&H264) && !has(&DISC_CODEC) || has(&H265) || tags.to_ascii_lowercase().contains("rip");
    let disc = match source {
        Some(Source::Bluray) => {
            !remux
                && (has(&COMPLETE)
                    || has(&ISO)
                    || has(&DISC_SIZE)
                    || (has(&DISC_CODEC) && !encoded))
        }
        Some(Source::Dvd) => has(&DVD_DISC),
        _ => false,
    };
    (remux, source, disc)
}

/// The video codec the tags name.
pub(super) fn video_codec(has: &impl Fn(&Regex) -> bool) -> Option<&'static str> {
    [
        (&*H265, "h265"),
        (&*H264, "h264"),
        (&*AV1, "av1"),
        (&*XVID, "xvid"),
        (&*MPEG2, "mpeg2"),
    ]
    .into_iter()
    .find(|(r, _)| has(r))
    .map(|(_, codec)| codec)
}

/// The audio codec the tags name.
pub(super) fn audio_codec(has: &impl Fn(&Regex) -> bool) -> Option<&'static str> {
    [
        (&*TRUEHD, "truehd"),
        (&*DTS_X, "dts-x"),
        (&*DTS_HRA, "dts-hd hra"),
        (&*DTS_HD, "dts-hd ma"),
        (&*DTS, "dts"),
        (&*EAC3, "eac3"),
        (&*AC3, "ac3"),
        (&*FLAC, "flac"),
        (&*OPUS, "opus"),
        (&*PCM, "pcm"),
        (&*AAC, "aac"),
        (&*MP3, "mp3"),
    ]
    .into_iter()
    .find(|(r, _)| has(r))
    .map(|(_, codec)| codec)
}

/// The audio channels the tags name ("5.1"), from a layout or a channel count.
pub(super) fn audio_channels(tags: &str) -> Option<String> {
    CHANNELS
        .captures(tags)
        .map(|c| format!("{}.{}", &c[1], &c[2]))
        .or_else(|| {
            CHANNEL_COUNT.captures(tags).map(|c| match &c[1] {
                "8" => "7.1".to_owned(),
                "6" => "5.1".to_owned(),
                _ => "2.0".to_owned(),
            })
        })
}

/// The audio languages, the subtitle languages, and the tags left once both are taken out.
pub(super) fn languages(
    tags: &str,
    multi_subs: bool,
    has: &impl Fn(&Regex) -> bool,
) -> (Vec<&'static str>, Vec<&'static str>, String) {
    let mut subtitles: Vec<&'static str> = Vec::new();
    let mut audio_tags = tags.to_owned();
    for (_, subtitle, code) in LANGUAGES.iter() {
        if subtitle.is_match(&audio_tags) {
            subtitles.push(code);
            audio_tags = subtitle.replace_all(&audio_tags, " ").into_owned();
        }
    }
    if has(&VOSTFR) && !subtitles.contains(&"fr") {
        subtitles.push("fr");
    }
    for r in [&*VOSTFR, &*MULTI_SUBS] {
        audio_tags = r.replace_all(&audio_tags, " ").into_owned();
    }
    let mut languages = Vec::new();
    for (audio, _, code) in LANGUAGES.iter() {
        if audio.is_match(&audio_tags) {
            audio_tags = audio.replace_all(&audio_tags, " ").into_owned();
            if multi_subs {
                if !subtitles.contains(code) {
                    subtitles.push(code);
                }
            } else {
                languages.push(*code);
            }
        }
    }
    if DUB.is_match(&audio_tags) && !languages.contains(&"en") {
        languages.push("en");
    }
    (languages, subtitles, audio_tags)
}

/// The anime re-release version: "v2" in the tags, or after an episode number.
pub(super) fn version(tags: &str, episodes: &Episodes, rest: &str) -> Option<u8> {
    VERSION
        .captures(tags)
        .and_then(|c| c[1].parse().ok())
        .or(match episodes {
            Episodes::Absolute(_) | Episodes::Standard { .. } => ABSOLUTE
                .captures(rest)
                .and_then(|c| c.name("version")?.as_str().parse().ok()),
            _ => None,
        })
}

/// PROPER, REPACK and RERIP, with REPACK2 counting two.
pub(super) fn propers(tags: &str) -> u8 {
    let total: u32 = tags
        .split(is_separator)
        .filter_map(|w| PROPER_WORD.captures(w))
        .map(|c| c.get(1).and_then(|n| n.as_str().parse().ok()).unwrap_or(1))
        .sum();
    total.min(u8::MAX as u32) as u8
}
