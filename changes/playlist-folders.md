### Added

- Playlist folders through the API: list, make, rename and delete your folders, and file a
  playlist in one (`folder_id` on `PUT /playlists/{id}`), each written into the sync log as an
  Aoide device writes it. Deleting a folder moves what was in it up a level.
- Adding a song the library doesn't have yet to a playlist, by your own request for it
  (`POST /playlists/{id}/waiting`): it waits there until the song arrives.
