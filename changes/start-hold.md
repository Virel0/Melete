### Changed

- The background work that asks MusicBrainz waits a few minutes after the server starts, each
  task its own time (music matching 2 minutes in, playlist imports 3, followed artists 4,
  library artists' portraits 6), and a portraits run spreads its lookups 15 seconds apart,
  rather than everything asking at once while the server came up and being told to wait.
