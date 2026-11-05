### Added

- **Make room suggestions** (`GET /api/v1/admin/make-room`, admins): movies and series to
  delete, swap for a smaller release, or compress, the strongest first, each with the space it
  would free and why, and the total. Nothing anyone is watching, has on their list or requested
  lately is suggested; a major studio's titles are suggested less (Settings → Make room adds
  studios to the built-in list). Suggestions only: acting on them comes later.
- Titles' companies from TMDB (studios and networks), kept with their details; titles fetched
  before are asked for theirs once.
