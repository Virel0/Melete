### Added

- Portraits for the library's artists. Music artist items gain `portrait`, served from the new
  `GET /music/artists/{id}/portrait` (at the album covers' sizes), and `portrait_credit`. An
  artist whose albums all sit in one folder holding artist.jpg, artist.png, folder.jpg or
  folder.png has that picture, whatever the settings, after the next scan. Otherwise, with
  artists' portraits on (`MELETE_ARTIST_PORTRAITS`), artists with a MusicBrainz id are looked up
  on Wikimedia Commons in the background, 20 an hour (the new `artist-portraits` task), and
  their portraits are kept while they're in the library.
