-- Whether a cached release was fetched with each track's artist credit, which releases cached
-- before those were kept lack (their tracks' `artist_credit` is NULL). Matching fetches those
-- again once, at MusicBrainz's pace, as it does releases fetched before works were kept.
-- Releases cached from now on have them, so the default is 1; the ones already cached without
-- a single track credit are marked 0.
ALTER TABLE mb_releases ADD COLUMN with_track_credits INTEGER NOT NULL DEFAULT 1
    CHECK (with_track_credits IN (0, 1));
UPDATE mb_releases SET with_track_credits = 0
WHERE NOT EXISTS (SELECT 1 FROM mb_release_tracks t
                  WHERE t.release_id = mb_releases.id AND t.artist_credit IS NOT NULL);
