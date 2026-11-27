use crate::probe::{AudioStream, SubtitleStream};

use super::*;

fn audio(language: Option<&str>) -> AudioStream {
    AudioStream {
        index: 1,
        codec: "aac".into(),
        profile: None,
        channels: Some(2),
        channel_layout: None,
        language: language.map(str::to_owned),
        title: None,
        default: true,
    }
}

fn subtitle(language: Option<&str>, title: Option<&str>, forced: bool) -> SubtitleStream {
    SubtitleStream {
        index: 2,
        codec: "ass".into(),
        language: language.map(str::to_owned),
        title: title.map(str::to_owned),
        default: false,
        forced,
        text: true,
    }
}

fn info(audio: Vec<AudioStream>, subtitles: Vec<SubtitleStream>) -> MediaInfo {
    MediaInfo {
        container: None,
        duration_seconds: Some(1440.0),
        bit_rate: None,
        video: Vec::new(),
        audio,
        subtitles,
    }
}

fn english() -> Language {
    Language::parse("en").unwrap()
}

#[test]
fn japanese_audio_needs_english_subtitles_inside_or_beside() {
    let jp = || vec![audio(Some("jpn"))];
    let check =
        |info: &MediaInfo, beside: &[&str]| lacking("Show - 01.mkv", info, beside, &english());
    // Subtitled in English: fine.
    assert_eq!(
        check(
            &info(jp(), vec![subtitle(Some("eng"), Some("Full"), false)]),
            &[]
        ),
        None
    );
    // A raw: no subtitles at all.
    let raw = check(&info(jp(), Vec::new()), &[]).unwrap();
    assert!(raw.contains("jpn audio and no subtitles"), "{raw}");
    // Subtitles, but none in English.
    let others = check(
        &info(
            jp(),
            vec![
                subtitle(Some("spa"), None, false),
                subtitle(Some("por"), None, false),
            ],
        ),
        &[],
    )
    .unwrap();
    assert!(others.contains("no en subtitles"), "{others}");
    // English signs and songs only: what's said isn't translated.
    assert!(
        check(
            &info(
                jp(),
                vec![subtitle(Some("eng"), Some("Signs & Songs"), false)]
            ),
            &[]
        )
        .is_some()
    );
    assert!(check(&info(jp(), vec![subtitle(Some("eng"), None, true)]), &[]).is_some());
    // English beside it, named or not; another language beside it doesn't count.
    assert_eq!(check(&info(jp(), Vec::new()), &["Show - 01.en.ass"]), None);
    assert_eq!(check(&info(jp(), Vec::new()), &["Show - 01.eng.srt"]), None);
    assert_eq!(
        check(&info(jp(), Vec::new()), &["Show - 01 [English].ass"]),
        None
    );
    assert_eq!(
        check(&info(jp(), Vec::new()), &["Show - 01.ass"]),
        None,
        "a name saying no language counts"
    );
    assert!(check(&info(jp(), Vec::new()), &["Show - 01.es.srt"]).is_some());
    assert!(check(&info(jp(), Vec::new()), &["Show - 01.en.forced.srt"]).is_some());
}

#[test]
fn english_audio_or_an_untagged_track_counts_and_other_titles_dont() {
    let check = |info: &MediaInfo| lacking("Show - 01.mkv", info, &[], &english());
    // Dubbed: no subtitles needed.
    assert_eq!(
        check(&info(
            vec![audio(Some("jpn")), audio(Some("eng"))],
            Vec::new()
        )),
        None
    );
    // Untagged audio isn't taken for English.
    assert!(check(&info(vec![audio(None)], Vec::new())).is_some());
    // An untagged subtitle track counts, unless its title names another language.
    assert_eq!(
        check(&info(
            vec![audio(Some("jpn"))],
            vec![subtitle(None, None, false)]
        )),
        None
    );
    assert_eq!(
        check(&info(
            vec![audio(Some("jpn"))],
            vec![subtitle(None, Some("Full Subs [Group]"), false)]
        )),
        None
    );
    assert_eq!(
        check(&info(
            vec![audio(Some("jpn"))],
            vec![subtitle(None, Some("English"), false)]
        )),
        None
    );
    assert!(
        check(&info(
            vec![audio(Some("jpn"))],
            vec![subtitle(None, Some("Español (Latino)"), false)]
        ))
        .is_some()
    );
}
