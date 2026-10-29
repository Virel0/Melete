-- Seek-bar previews made for each file version: the sheets and their index are in the data
-- folder's trickplay/<file id>-<size>-<modified>/. A file ffmpeg couldn't read keeps why, and
-- is looked at again only once it changes.
CREATE TABLE media_trickplay (
    file_id INTEGER PRIMARY KEY REFERENCES media_files (id),
    size INTEGER NOT NULL,
    modified_at INTEGER NOT NULL,
    made_at INTEGER NOT NULL,
    error TEXT
) STRICT;
