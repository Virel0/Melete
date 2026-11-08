### Added

- Series can follow one of TMDB's episode groups instead of TMDB's own order, for shows TMDB
  lists as one long season while the files are split into seasons. When a series' files use
  seasons TMDB's own order doesn't have and exactly one group has exactly their seasons and
  episodes, Melete follows it by itself and says so in the series' history; otherwise the
  closest group is suggested. Series details carry `episode_order`.

### Fixed

- Episodes of a show TMDB lists as one long season no longer show as Untitled, Missing and
  Monitored when the files are split into seasons and a group fits them.
