-- Whether a series asked for is anime, as TMDB said when it was asked for (animated and
-- Japanese), so a request waiting for a decision can be counted and listed under Anime rather
-- than Series before it's on the wanted list. NULL for movies and music, and for a series asked
-- for before this was kept: those count as anime only when a wanted series or a folder for it
-- says so, as below.
ALTER TABLE requests ADD COLUMN anime INTEGER CHECK (anime IN (0, 1));

-- Series asked for before: anime when the wanted list has it as anime, or a folder of it sits in
-- a library with the anime category. The rest stay unknown and count as series.
UPDATE requests SET anime = 1
WHERE kind = 'series'
  AND (EXISTS (SELECT 1 FROM title_event_titles t JOIN wanted_series w ON w.id = t.wanted_id
               WHERE t.kind = 'series' AND t.service = 'anime' AND w.tmdb_id = requests.tmdb_id)
       OR EXISTS (SELECT 1 FROM series s JOIN libraries l ON l.id = s.library_id
                  WHERE l.category = 'anime'
                    AND coalesce(s.match_tmdb_id, s.tmdb_id) = requests.tmdb_id));
