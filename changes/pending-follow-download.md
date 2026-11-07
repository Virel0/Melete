### Fixed

- A playlist item waiting on a song request fills once the request's download is imported,
  even when the file is tagged with another MusicBrainz recording of the song, or none: the one
  track the download put in the library counts as the song, as the request already showed it
  in the library.
