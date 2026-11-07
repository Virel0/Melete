-- Each user's own languages for one kind of title (movies, shows or anime), used in place of
-- their languages (user_languages) for that kind; a kind with no row follows those.
CREATE TABLE user_kind_languages (
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('movies', 'shows', 'anime')),
    audio_languages TEXT NOT NULL CHECK (json_valid(audio_languages)
        AND json_type(audio_languages) = 'array'),
    subtitle_languages TEXT NOT NULL CHECK (json_valid(subtitle_languages)
        AND json_type(subtitle_languages) = 'array'),
    subtitle_mode TEXT NOT NULL CHECK (subtitle_mode IN ('off', 'forced', 'foreign', 'always')),
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, kind)
) STRICT;
