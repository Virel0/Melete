### Added

- Series can follow one of TMDB's episode groups instead of TMDB's own order, for shows TMDB
  lists as one long season while the files are split into seasons. When a series' files use
  seasons TMDB's own order doesn't have and exactly one group has exactly their seasons and
  episodes, Melete follows it by itself and says so on the series' page and in its history;
  otherwise the closest group is suggested, with Use this order for admins. Admins choose any
  group, or TMDB's own order, under Episode order on the series' Manage page
  (`GET /api/v1/series/{id}/orders`, `PUT /api/v1/series/{id}/order`); a choice by hand is never
  changed automatically. Series details carry `episode_order`.

### Fixed

- Episodes of a show TMDB lists as one long season no longer show as Untitled, Missing and
  Monitored when the files are split into seasons, and an episode numbered from the first
  ("Show - 30") that the library has under the group isn't grabbed again.
