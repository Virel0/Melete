### Changed

- When Melete starts, it looks at each library's title folders' times (not a full scan) once
  the first library check is in, and scans the folders changed while it was down: on their
  own, or the library whole when a title folder is new at the top or a music library changed.
  A library that isn't responding, or was never scanned, is left alone, and nothing is marked
  missing by the look itself.
- Writes to the database wait their turn for up to 5 seconds instead of failing at once with
  "database is locked" when another write is under way. A scan whose record still finds the
  database busy is tried again twice before giving up.

### Fixed

- Swapping arrived songs into playlists read every library track once for each song request
  still pending in a playlist; it now looks up each request's downloads directly. On a
  test database with tens of thousands of tracks and a few hundred pending requests, that
  went from seconds to milliseconds.
- The first scan after a start waits for the first library check, so a library is known to
  answer before anything scans it.
