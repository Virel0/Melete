### Added

- An admin can choose the YouTube video for a wanted song by hand (Manage → Music → wanted
  songs → Use a YouTube video, or `POST /api/v1/wanted/songs/{id}/youtube` with its link): for a
  song YouTube's matching couldn't find, it's fetched as the song without matching it or
  checking its length. Only while songs from YouTube are on.
