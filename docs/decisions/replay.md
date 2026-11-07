Decision: Replay counts a listen as a play as the Aoide apps do: it reached the end, or it
wasn't skipped, ended, and ran at least half the track or four minutes, whichever is less (four
minutes when the length isn't known). Time listened counts every listen, skips too. The server
works the figures out (`GET /music/replay`), with the busiest day on the listener's clock, and the
top lists hold only what the listener may see. Deleting listens from Replay deletes them from
Melete's history only.

Why: The same song should count the same on the phone, the desk and the web; working it out on
the server keeps the browser from paging the whole history.

Date: 2026-11-06
