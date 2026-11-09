-- The episodes of its season a season pack was taken for, when part of the season was on disk or
-- on its way already: only the pack's files for these episodes are brought home. NULL takes the
-- whole download, as before. A list of one or more episode numbers, only for a download of a
-- season that names no one episode.
ALTER TABLE downloads ADD COLUMN episodes TEXT CHECK (
    episodes IS NULL
    OR (season IS NOT NULL AND episode IS NULL
        AND json_valid(episodes) AND json_type(episodes) = 'array'
        AND json_array_length(episodes) > 0
        AND episodes NOT GLOB '*[^][0-9, ]*'
        AND episodes NOT GLOB '*[[]*[[]*')
);
