-- A pack of several seasons brings home every monitored season it adds to, not only the one it
-- was grabbed for: those others, each with the episodes it's taken for, as
-- {"<season>": [episodes]}. The season it was grabbed for stays in season and episodes.
ALTER TABLE downloads ADD COLUMN seasons TEXT CHECK (seasons IS NULL OR json_valid(seasons));
