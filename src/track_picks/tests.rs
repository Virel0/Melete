use super::*;
use crate::probe::AudioStream;

fn lang(code: &str) -> Language {
    Language::parse(code).unwrap()
}

fn langs(codes: &[&str]) -> Vec<Language> {
    codes.iter().map(|c| lang(c)).collect()
}

fn audio(index: i64, language: Option<&str>, default: bool) -> AudioStream {
    AudioStream {
        index,
        codec: "aac".into(),
        profile: None,
        channels: Some(2),
        channel_layout: None,
        language: language.map(Into::into),
        title: None,
        default,
    }
}

fn media(audio: Vec<AudioStream>) -> MediaInfo {
    MediaInfo {
        container: Some("matroska,webm".into()),
        duration_seconds: Some(1400.0),
        bit_rate: None,
        video: Vec::new(),
        audio,
        subtitles: Vec::new(),
    }
}

/// A subtitle track: `flags` holds f (forced), h (hearing impaired), d (default), p (pictures).
fn sub(track: &str, language: Option<&str>, flags: &str) -> SubtitleTrack {
    SubtitleTrack {
        track: track.into(),
        language: language.map(lang),
        default: flags.contains('d'),
        forced: flags.contains('f'),
        hearing_impaired: flags.contains('h'),
        text: !flags.contains('p'),
    }
}

fn prefs(audio: &[&str], subtitles: &[&str], mode: SubtitleMode) -> Preferences {
    Preferences {
        audio: langs(audio),
        subtitles: langs(subtitles),
        mode,
    }
}

/// (audio, its reason, subtitle, its reason)
fn picked(
    p: &Preferences,
    choice: &SeriesChoice,
    m: &MediaInfo,
    subs: &[SubtitleTrack],
) -> (Option<i64>, &'static str, Option<String>, &'static str) {
    let pick = pick(p, choice, Some(m), subs);
    (
        pick.audio,
        pick.audio_reason,
        pick.subtitle,
        pick.subtitle_reason,
    )
}

/// An anime episode: an English dub (the default) and the Japanese audio; English signs
/// (forced), full English subtitles, and English for the hearing impaired.
fn anime() -> (MediaInfo, Vec<SubtitleTrack>) {
    (
        media(vec![
            audio(1, Some("eng"), true),
            audio(2, Some("jpn"), false),
        ]),
        vec![
            sub("s3", Some("eng"), "fd"),
            sub("s4", Some("eng"), "h"),
            sub("s5", Some("eng"), ""),
        ],
    )
}

#[test]
fn without_languages_the_file_default_plays_with_its_forced_subtitles() {
    let (m, subs) = anime();
    let none = SeriesChoice::default();
    assert_eq!(
        picked(&Preferences::default(), &none, &m, &subs),
        (Some(1), "file_default", Some("s3".into()), "forced")
    );
    let plain = [sub("s5", Some("eng"), "")];
    assert_eq!(
        picked(&Preferences::default(), &none, &m, &plain),
        (Some(1), "file_default", None, "no_match")
    );
}

#[test]
fn foreign_audio_gets_full_subtitles_and_understood_audio_only_forced_ones() {
    let (m, subs) = anime();
    let none = SeriesChoice::default();
    // Japanese audio isn't one of the subtitle languages: full English, not the signs and not
    // the hearing-impaired track.
    let japanese = prefs(&["ja"], &["en"], SubtitleMode::Foreign);
    assert_eq!(
        picked(&japanese, &none, &m, &subs),
        (
            Some(2),
            "preferred_language",
            Some("s5".into()),
            "foreign_audio"
        )
    );
    // The dub is understood: only the signs.
    let dub = prefs(&["en"], &["en"], SubtitleMode::Foreign);
    assert_eq!(
        picked(&dub, &none, &m, &subs),
        (Some(1), "preferred_language", Some("s3".into()), "forced")
    );
    let no_signs = [sub("s5", Some("eng"), "")];
    assert_eq!(
        picked(&dub, &none, &m, &no_signs),
        (Some(1), "preferred_language", None, "audio_understood")
    );
}

#[test]
fn each_mode_shows_what_it_says() {
    let (m, subs) = anime();
    let none = SeriesChoice::default();
    let with = |mode| picked(&prefs(&["en"], &["en"], mode), &none, &m, &subs);
    assert_eq!(with(SubtitleMode::Off).2, None);
    assert_eq!(with(SubtitleMode::Off).3, "off");
    assert_eq!(with(SubtitleMode::Forced).2.as_deref(), Some("s3"));
    assert_eq!(with(SubtitleMode::Always).2.as_deref(), Some("s5"));
    assert_eq!(with(SubtitleMode::Always).3, "always");
    // Forced only, with the audio in another language: the signs in the subtitle language.
    let forced_only = prefs(&["ja"], &["en"], SubtitleMode::Forced);
    assert_eq!(
        picked(&forced_only, &none, &m, &subs).2.as_deref(),
        Some("s3")
    );
}

#[test]
fn untagged_tracks_are_never_a_match() {
    let m = media(vec![audio(1, None, true), audio(2, Some("fre"), false)]);
    let subs = [sub("s3", None, ""), sub("s4", Some("en"), "")];
    let p = prefs(&["en"], &["en"], SubtitleMode::Foreign);
    // English isn't there: the default plays, and as its language isn't known it isn't
    // understood, so full English subtitles show; the untagged track never counts as English.
    assert_eq!(
        picked(&p, &SeriesChoice::default(), &m, &subs),
        (Some(1), "file_default", Some("s4".into()), "foreign_audio")
    );
    let untagged_only = [sub("s3", None, "")];
    assert_eq!(
        picked(&p, &SeriesChoice::default(), &m, &untagged_only).3,
        "no_match"
    );
}

#[test]
fn a_region_never_stands_in_for_another() {
    let m = media(vec![audio(1, Some("jpn"), true)]);
    let p = prefs(&[], &["pt-BR"], SubtitleMode::Foreign);
    let none = SeriesChoice::default();
    let all = [
        sub("x1", Some("pt-pt"), ""),
        sub("x2", Some("por"), ""),
        sub("x3", Some("pt-br"), ""),
    ];
    assert_eq!(picked(&p, &none, &m, &all).2.as_deref(), Some("x3"));
    // A track naming no region may be either, so it's next best.
    assert_eq!(picked(&p, &none, &m, &all[..2]).2.as_deref(), Some("x2"));
    assert_eq!(picked(&p, &none, &m, &all[..1]).2, None);
}

#[test]
fn text_is_picked_over_pictures_in_one_language() {
    let m = media(vec![audio(1, Some("jpn"), true)]);
    let p = prefs(&[], &["en"], SubtitleMode::Foreign);
    let subs = [sub("s2", Some("eng"), "pd"), sub("x1", Some("en"), "")];
    let none = SeriesChoice::default();
    assert_eq!(picked(&p, &none, &m, &subs).2.as_deref(), Some("x1"));
}

#[test]
fn the_series_choice_comes_first_and_falls_back_when_its_track_is_missing() {
    let (m, subs) = anime();
    let p = prefs(&["ja"], &["en"], SubtitleMode::Foreign);
    let dub_and_sdh = SeriesChoice {
        audio: Some(lang("en")),
        subtitles: Some(SubtitleChoice::On {
            language: lang("en"),
            forced: false,
            hearing_impaired: true,
        }),
    };
    assert_eq!(
        picked(&p, &dub_and_sdh, &m, &subs),
        (Some(1), "series_choice", Some("s4".into()), "series_choice")
    );
    let off = SeriesChoice {
        audio: None,
        subtitles: Some(SubtitleChoice::Off),
    };
    assert_eq!(
        picked(&p, &off, &m, &subs),
        (Some(2), "preferred_language", None, "series_choice")
    );
    // French was chosen, and this episode has none: the user's languages decide.
    let french = SeriesChoice {
        audio: Some(lang("fr")),
        subtitles: Some(SubtitleChoice::On {
            language: lang("fr"),
            forced: false,
            hearing_impaired: false,
        }),
    };
    assert_eq!(
        picked(&p, &french, &m, &subs),
        (
            Some(2),
            "preferred_language",
            Some("s5".into()),
            "foreign_audio"
        )
    );
}

#[test]
fn a_file_without_tracks_says_so() {
    let m = media(Vec::new());
    let pick = pick(
        &Preferences::default(),
        &SeriesChoice::default(),
        Some(&m),
        &[],
    );
    assert_eq!(
        (pick.audio, pick.audio_reason, pick.subtitle_reason),
        (None, "no_tracks", "no_tracks")
    );
}
