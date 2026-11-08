### Fixed

- A new episode no longer keeps TMDB's placeholder name ("Episode 6") for up to a week. A
  series with an episode aired in the last 14 days, or airing within a day, that has no
  title, the placeholder title or no summary is fetched again every 6 hours until TMDB fills
  it in, whether it's in the library or wanted. A file that comes in for an episode still
  untitled has its series fetched again at once, on the refresh after the scan. Only the
  series' TMDB id is sent, as with every refresh.
