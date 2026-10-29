-- Requests: anyone signed in asks for a title; it joins the wanted list once approved,
-- or at once for a user whose requests are let through. A request remembers what it became
-- on the wanted list, so its requester can follow it there.
CREATE TABLE requests (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('movie', 'series', 'artist', 'album', 'song')),
    -- A movie or series by its TMDB id, and for a series one season, or NULL for all of it.
    tmdb_id INTEGER,
    season INTEGER CHECK (season IS NULL OR season >= 0),
    -- Music by its MusicBrainz ids: the artist, an album's release group, a song's recording.
    mb_artist_id TEXT,
    mb_release_group_id TEXT,
    mb_recording_id TEXT,
    -- As shown: the title, its year, and for music its artist.
    title TEXT NOT NULL,
    year INTEGER,
    artist TEXT,
    -- TMDB's poster path, or a cover's address, for showing it.
    poster TEXT,
    state TEXT NOT NULL CHECK (state IN ('requested', 'approved', 'declined')),
    -- Why it was declined.
    reason TEXT,
    wanted_movie_id INTEGER REFERENCES wanted_movies (id) ON DELETE SET NULL,
    wanted_series_id INTEGER REFERENCES wanted_series (id) ON DELETE SET NULL,
    wanted_artist_id INTEGER REFERENCES wanted_artists (id) ON DELETE SET NULL,
    wanted_album_id INTEGER REFERENCES wanted_albums (id) ON DELETE SET NULL,
    wanted_song_id INTEGER REFERENCES wanted_songs (id) ON DELETE SET NULL,
    created_at INTEGER NOT NULL,
    decided_at INTEGER,
    -- NULL for one approved at once, as something already wanted is.
    decided_by INTEGER REFERENCES users (id) ON DELETE SET NULL,
    CHECK ((kind IN ('movie', 'series')) = (tmdb_id IS NOT NULL)),
    CHECK (kind = 'series' OR season IS NULL),
    CHECK (kind != 'artist' OR mb_artist_id IS NOT NULL),
    CHECK (kind != 'album' OR mb_release_group_id IS NOT NULL),
    CHECK (state = 'declined' OR reason IS NULL),
    CHECK ((state = 'requested') = (decided_at IS NULL))
) STRICT;
CREATE INDEX requests_by_user ON requests (user_id, created_at);
CREATE INDEX requests_by_state ON requests (state);
-- One open request a user has for the same thing; a declined one can be asked again.
CREATE UNIQUE INDEX requests_once ON requests (
    user_id, kind, coalesce(tmdb_id, -1), coalesce(season, -1),
    coalesce(mb_recording_id, mb_release_group_id, mb_artist_id, lower(title || '␟' || coalesce(artist, '')))
) WHERE state != 'declined';

-- A user whose requests skip approval.
ALTER TABLE users ADD COLUMN requests_approved INTEGER NOT NULL DEFAULT 0
    CHECK (requests_approved IN (0, 1));

-- Where approved requests go, by kind; series that are anime have their own, as anime often
-- has a library of its own.
CREATE TABLE request_defaults (
    kind TEXT PRIMARY KEY CHECK (kind IN ('movies', 'series', 'anime', 'music')),
    library_id INTEGER REFERENCES libraries (id) ON DELETE SET NULL,
    profile_id INTEGER REFERENCES quality_profiles (id) ON DELETE SET NULL,
    music_profile_id INTEGER REFERENCES music_profiles (id) ON DELETE SET NULL,
    CHECK ((kind = 'music') OR music_profile_id IS NULL),
    CHECK ((kind != 'music') OR profile_id IS NULL)
) STRICT;
