-- Following artists for their new releases: when each followed artist's studio albums
-- were last looked up on MusicBrainz, and every release group seen there, so one that appears
-- later is told from the ones already known.
CREATE TABLE followed_artist_checks (
    wanted_artist_id INTEGER PRIMARY KEY REFERENCES wanted_artists (id) ON DELETE CASCADE,
    -- The day of the first look, YYYY-MM-DD: "new" wants what's released from then on.
    since TEXT NOT NULL,
    -- Unix seconds.
    checked_at INTEGER NOT NULL,
    -- Whether a look has ever succeeded: until one has, nothing seen counts as new.
    looked INTEGER NOT NULL CHECK (looked IN (0, 1)),
    -- Why the last look failed, when it did.
    error TEXT
) STRICT;

CREATE TABLE followed_release_groups (
    wanted_artist_id INTEGER NOT NULL REFERENCES wanted_artists (id) ON DELETE CASCADE,
    mb_release_group_id TEXT NOT NULL,
    -- Unix seconds.
    seen_at INTEGER NOT NULL,
    PRIMARY KEY (wanted_artist_id, mb_release_group_id)
) STRICT;
