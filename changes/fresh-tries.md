### Fixed

- A wanted song with no Soulseek search on record (its first search couldn't start, or its
  searches were cleared after 30 days) is searched for, up to 60 a run; before, nothing looked
  for it again, and its playlist items said "On its way" for good.
- A song the library already has (by its recording, or one track with its title and artist,
  under another of MusicBrainz's listings or none) is swapped into the playlists waiting for
  it, and isn't fetched again from Soulseek or YouTube.
- A playlist's pending song whose request was approved but that wasn't found anywhere says
  "Not found yet" (state `not_found`, with why) instead of "On its way".
- Songs and albums given up on before this release are tried again: their Soulseek tries
  start over, and songs YouTube had nothing fitting for, or failed to fetch, are tried there
  again with today's matching (migration 0164).
