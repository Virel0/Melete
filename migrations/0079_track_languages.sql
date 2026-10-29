-- Each user's audio and subtitle languages, and per series the tracks they last chose by hand,
-- so every app plays a file with the same tracks.

CREATE TABLE user_languages (
    user_id INTEGER PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    -- JSON arrays of language codes ("ja", "pt-BR"), most wanted first.
    audio_languages TEXT NOT NULL CHECK (json_valid(audio_languages)
        AND json_type(audio_languages) = 'array'),
    subtitle_languages TEXT NOT NULL CHECK (json_valid(subtitle_languages)
        AND json_type(subtitle_languages) = 'array'),
    subtitle_mode TEXT NOT NULL CHECK (subtitle_mode IN ('off', 'forced', 'foreign', 'always')),
    updated_at INTEGER NOT NULL
) STRICT;

CREATE TABLE series_track_choices (
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    series_id INTEGER NOT NULL REFERENCES series (id),
    -- NULL when the audio chosen had no language tag.
    audio_language TEXT,
    -- 'unknown' when the subtitles chosen had no language tag.
    subtitles TEXT NOT NULL CHECK (subtitles IN ('unknown', 'off', 'on')),
    subtitle_language TEXT,
    subtitle_forced INTEGER NOT NULL DEFAULT 0 CHECK (subtitle_forced IN (0, 1)),
    subtitle_hearing_impaired INTEGER NOT NULL DEFAULT 0
        CHECK (subtitle_hearing_impaired IN (0, 1)),
    chosen_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, series_id),
    CHECK ((subtitles = 'on') = (subtitle_language IS NOT NULL))
) STRICT;
