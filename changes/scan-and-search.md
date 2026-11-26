### Added

- Scan and search, for each movie or series library under Settings, Libraries: the library is
  scanned (its new files matched and probed), then, as ticked, its missing films or episodes
  are searched for, then upgrades of its files below their profile's cutoff, then the
  subtitles its files lack are fetched in the languages set for imports. Each search step is a
  Search all missing run for that library's titles alone, paced and followed the same way
  (`POST`/`GET /api/v1/libraries/{name}/scan-and-search`; a missing search's `library_id`
  says which library it was for).
