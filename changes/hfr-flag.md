### Added

- A movie whose copy plays at 48 fps or more is noted on its page. When the film isn't one high
  frame rate releases are wanted for, the note says it's most likely made up from the film's 24
  and may stutter, with a link for admins to search for another release, and Manage's health
  lists it. Apps get it as `high_frame_rate` (`{fps, allowed}`) on `GET /api/v1/movies/{id}`.
- Each movie's Manage page has a switch to allow releases at a high frame rate for that film,
  for films really made so; `GET /api/v1/high-frame-rate-films` and
  `PUT /api/v1/high-frame-rate-films/{tmdb_id}` do the same for apps.
