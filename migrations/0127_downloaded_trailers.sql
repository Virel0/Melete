-- Downloaded trailers. First, each library title's YouTube trailers and teasers from TMDB's video
-- list, fetched only while trailer downloads are on, after its details and again whenever they
-- are: a JSON array of {key, name, language, kind, official, size, published_at}; NULL until
-- fetched. Which one a title gets is chosen from it.
ALTER TABLE tmdb_movies ADD COLUMN videos TEXT CHECK (videos IS NULL OR json_valid(videos));
ALTER TABLE tmdb_movies ADD COLUMN videos_fetched_at INTEGER;
ALTER TABLE tmdb_series ADD COLUMN videos TEXT CHECK (videos IS NULL OR json_valid(videos));
ALTER TABLE tmdb_series ADD COLUMN videos_fetched_at INTEGER;

-- Each title's downloaded trailer: the YouTube video chosen for it and the most it was asked for
-- at (720, 1080 or 2160 lines), and how that went. A title's row goes when it leaves the library
-- or gets a trailer of its own in its folder; the video is kept in Melete's config folder.
CREATE TABLE downloaded_trailers (
    id INTEGER PRIMARY KEY,
    movie_id INTEGER UNIQUE REFERENCES movies (id) ON DELETE CASCADE,
    series_id INTEGER UNIQUE REFERENCES series (id) ON DELETE CASCADE,
    video TEXT NOT NULL CHECK (length(video) = 11),
    max_height INTEGER NOT NULL CHECK (max_height IN (720, 1080, 2160)),
    state TEXT NOT NULL CHECK (state IN ('queued', 'done', 'unavailable', 'failed')),
    -- The video's name in the trailers folder, its height and length: the one downloaded last,
    -- kept while another is fetched in its place, and when that fails.
    file TEXT,
    height INTEGER,
    duration_seconds REAL,
    -- Why YouTube hasn't it, or why it failed.
    reason TEXT,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    queued_at INTEGER NOT NULL,
    finished_at INTEGER,
    CHECK ((movie_id IS NULL) <> (series_id IS NULL)),
    CHECK (state <> 'done' OR file IS NOT NULL),
    CHECK ((state IN ('unavailable', 'failed')) = (reason IS NOT NULL)),
    CHECK ((state = 'queued') = (finished_at IS NULL))
) STRICT;
CREATE INDEX downloaded_trailers_by_state ON downloaded_trailers (state);
