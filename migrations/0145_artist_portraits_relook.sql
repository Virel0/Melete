-- Commons began giving resized pictures on thumb.wikimedia.org, which Melete didn't take as a
-- picture, so every artist looked up since was kept as having no portrait, believed for a
-- week. Those are forgotten, so the next searches that list them look them up again (a few a
-- search, as always). Artists that truly have none are found to have none again.
DELETE FROM artist_portraits WHERE file IS NULL;
