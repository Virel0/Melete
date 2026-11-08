### Added

- Artists' portraits from Wikimedia Commons in searches for music to request, behind
  `MELETE_ARTIST_PORTRAITS` (Settings → Outside services → MusicBrainz; off by default, needs
  MusicBrainz on). An artist's `poster` in `GET /requests/search` is its Wikidata item's image,
  found through its MusicBrainz url relations, with its author, licence and Commons page in the
  new `poster_credit`; served from `GET /music/artist-portraits/{id}` (at the album covers'
  sizes) on discovery posters' terms. A search looks up at most three artists it doesn't know
  yet and keeps what it found, or that there's none, so later searches ask nothing. The web's
  search results show the credit under the picture.
