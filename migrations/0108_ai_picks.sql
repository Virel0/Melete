-- AI picks, kept so they're served at once and worked out again only now and then.

-- Each user's "Featured for you": the titles picked from what they watched, as library titles
-- with a short reason each ([{"kind": "movie", "id": 1, "reason": "..."}]).
CREATE TABLE ai_picks (
    user_id INTEGER PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    -- The titles watched they were picked from, as "m1,s4,…"; picks are made again when it
    -- changes.
    history TEXT NOT NULL,
    picks TEXT NOT NULL CHECK (json_valid(picks) AND json_type(picks) = 'array'),
    -- When picks were last made; NULL before they ever were.
    made_at INTEGER,
    -- When making them was last started, so it's started at most every few minutes.
    tried_at INTEGER NOT NULL,
    -- Why the last try failed; NULL when it didn't.
    error TEXT
) STRICT;

-- Titles like one title, shared by everyone who sees the same titles: `seen` is empty for
-- everyone who sees everything, else their libraries and highest rating ("l1,3;a12"), so a
-- title nobody asking may see is never sent as a candidate for them.
CREATE TABLE ai_similar (
    kind TEXT NOT NULL CHECK (kind IN ('movie', 'series')),
    title_id INTEGER NOT NULL,
    seen TEXT NOT NULL,
    picks TEXT NOT NULL CHECK (json_valid(picks) AND json_type(picks) = 'array'),
    made_at INTEGER,
    tried_at INTEGER NOT NULL,
    error TEXT,
    PRIMARY KEY (kind, title_id, seen)
) STRICT;
