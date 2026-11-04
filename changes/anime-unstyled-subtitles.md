### Added

- Anime episodes without styled subtitles (no ASS/SSA track in the file and no ASS/SSA file
  beside it) are noted in Anime's health, one note per series (`anime_unstyled_subtitles`),
  and marked on the series' Manage page. `GET /api/v1/series/{id}/unstyled-subtitles` lists
  them with the release group of each copy, and a series search takes `avoid_group` to turn
  that group's releases down; Manage's "Find a release with subtitles" searches so and lets an
  admin choose.
