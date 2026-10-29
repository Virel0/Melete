-- Subtitles looked for without anyone asking, for new imports lacking a language: one try per
-- file version and language, whether a subtitle made for the file was found or not, so the
-- same file isn't searched for again.
CREATE TABLE opensubtitles_import_tries (
    file_id INTEGER NOT NULL REFERENCES media_files (id),
    size INTEGER NOT NULL,
    modified_at INTEGER NOT NULL,
    language TEXT NOT NULL,
    tried_at INTEGER NOT NULL,
    fetched INTEGER NOT NULL CHECK (fetched IN (0, 1)),
    PRIMARY KEY (file_id, language)
) STRICT;
