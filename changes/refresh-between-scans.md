### Fixed

- Series due for a TMDB refresh are fetched again every hour between scans: a new episode TMDB
  hasn't named yet (every 6 hours), a wanted series still airing (daily) or a stale entry. A
  scan schedule of once a day no longer holds a new episode's title back until the next scan.
  Nothing is scanned or matched in between, and only the series' TMDB ids are sent.
