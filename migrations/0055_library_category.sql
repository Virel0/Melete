-- A library's category: anime or kids, for movies and shows libraries, so apps can tell an
-- anime library from the others without guessing from its name. NULL is no category.
ALTER TABLE libraries ADD COLUMN category TEXT
    CHECK (category IS NULL OR (category IN ('anime', 'kids') AND kind IN ('movies', 'shows')));
