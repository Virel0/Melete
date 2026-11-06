### Added

- A song's actions on the web have Not interested (kept out of mixes, radio, shuffle and smart
  playlists; set on the song playing, it moves on) and Don't count plays, the same flags the
  Aoide apps keep, so a change on any of them shows on the others.
- Hidden songs under Listen lists the flagged songs, each flag clearable one by one or all at
  once.
- `GET /music/flags` and `PUT /music/tracks/{id}/flags` read and set them, written into the
  listener's sync log as a device of theirs would write them.
