-- Motion covers made from albums' still covers, by the renderer beside Melete, for
-- albums without one of their own. One row per album, for the cover it was made from: `source`
-- is the cover as the colours job names it (`music_cover_colours.source`: which file, tag or
-- archive picture, and its bytes' hash), so a changed cover is made again; `version` is the
-- loop's, so changed loop settings make every one again.
CREATE TABLE music_motion_covers (
    album_id INTEGER PRIMARY KEY REFERENCES music_albums (id) ON DELETE CASCADE,
    state TEXT NOT NULL CHECK (state IN ('queued', 'done', 'skipped', 'failed')),
    source TEXT NOT NULL,
    version INTEGER NOT NULL,
    -- The video's name in the motion covers folder under the config folder, once done.
    file TEXT,
    -- Why it was skipped (mostly text, flat), or why it failed.
    reason TEXT,
    -- What the renderer said about it (size, timings), as JSON.
    details TEXT CHECK (details IS NULL OR json_valid(details)),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    queued_at INTEGER NOT NULL,
    finished_at INTEGER,
    CHECK ((state = 'done') = (file IS NOT NULL)),
    CHECK ((state IN ('skipped', 'failed')) = (reason IS NOT NULL)),
    CHECK ((state = 'queued') = (finished_at IS NULL))
) STRICT;
CREATE INDEX music_motion_covers_by_state ON music_motion_covers (state);

-- Each user's choice to see generated motion covers or not. A user who has never chosen has no
-- row, and sees them. Motion covers from an album's own folder are always shown.
CREATE TABLE music_motion_choices (
    user_id INTEGER PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    show_generated INTEGER NOT NULL CHECK (show_generated IN (0, 1)),
    -- Unix seconds.
    changed_at INTEGER NOT NULL
) STRICT;
