### Added

- A search page for movies and shows (Search in the palette, and its last result, See all
  results): the library's matches with their posters, then, while trending and similar titles
  are on, the titles TMDB has that aren't in the library, each with Ask for it.
- `GET /api/v1/discover/status`: whether trending and similar titles are on, asking nothing of
  TMDB, so an app can leave out what they give while they're off.
