### Added

- For You: songs for each listener from their own plays, some in the library to play and some
  not, to request, each saying which of their songs or artists it came from
  (`GET /api/v1/music/for-you`). Made in the background from ListenBrainz, which is sent only
  MusicBrainz ids; off by default (Settings → Outside services → MusicBrainz → For You from
  ListenBrainz, with MusicBrainz on). ListenBrainz can go through the proxy.
