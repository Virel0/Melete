-- For You, from ListenBrainz: each listener's list, kept so it's served at once and made again
-- only now and then, and ListenBrainz's answers, kept so a list made again doesn't ask again.
-- Both are forgotten when For You is turned off.

-- Each listener's list: songs in the library, by track id, and songs to request, by recording,
-- each with the seed it came from ([{"track_id": 1, "song": null, "because": {...}}, ...]).
CREATE TABLE for_you (
    user_id INTEGER PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    -- What their plays were when it was made ("count:last id"); made again when it changes.
    plays TEXT NOT NULL,
    items TEXT NOT NULL CHECK (json_valid(items) AND json_type(items) = 'array'),
    -- When it was last made; NULL before it ever was.
    made_at INTEGER,
    -- When making it was last started, so a failing one is tried again only now and then.
    tried_at INTEGER NOT NULL,
    -- Why the last try failed; NULL when it didn't.
    error TEXT
) STRICT;

-- ListenBrainz's answer about one MusicBrainz id: the recordings or artists listened to
-- alongside it, an artist's most listened-to recordings, or a recording's artists and album
-- (null when it knows none), as JSON.
CREATE TABLE listenbrainz_answers (
    kind TEXT NOT NULL
        CHECK (kind IN ('similar_recordings', 'similar_artists', 'top_recordings', 'recording')),
    mbid TEXT NOT NULL,
    answer TEXT NOT NULL CHECK (json_valid(answer)),
    fetched_at INTEGER NOT NULL,
    PRIMARY KEY (kind, mbid)
) STRICT;
