### Added

- Covers of music not in the library in searches for music to request, behind
  `MELETE_SEARCH_COVERS` (Settings → Outside services → MusicBrainz; off by default, needs
  MusicBrainz on). An album's `poster` in `GET /requests/search` is its release group's front
  cover from the Cover Art Archive, and a song's that of its single, else its album; served from
  `GET /music/release-groups/{id}/cover` (at the album covers' sizes) on discovery posters'
  terms: only those a search listed, for 30 days, 2000 at most, fetched once by the server and
  cached. The archive sees only the release group's id.
