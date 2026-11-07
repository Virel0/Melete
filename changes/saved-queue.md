### Added

- The web player keeps its queue across a reload: the queue, its place, shuffle, repeat and how
  far into the song, kept by the browser for the signed-in user only and restored paused; it's
  forgotten on Stop and on signing out.
- "Pick up from …" on Listen home, with the queue another of your devices was playing in the
  last day, as the Aoide apps offer it.
- `PUT /music/queue` publishes a session's queue as its device's `queue_state` row, so the Aoide
  apps can pick up from the web; the web player sends it as its queue changes and while it
  plays, never while listening privately.
