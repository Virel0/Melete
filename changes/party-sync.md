### Changed

- Watch parties keep players in step by speed rather than by seeking: drift under 0.3 s is
  left, up to 4 s a player plays slightly faster or slower until it's back, and only past 4 s
  does it seek, at most once in 10 s, never while it's buffering, and forward by what its last
  such seek took to play again. A player plays faster only with a few seconds buffered ahead.
- A play or pause in a party is sent at the party's place, not the player's own, so someone
  behind pausing no longer pulls everyone back; a seek the viewer made is still sent as theirs.
- The household player's playback report notes each change in how it keeps in step: drift,
  what it did (none, speed or seek), the party's place and whether it was buffering.
- The server log says when a watch party starts, ends, and is joined or left, and when a guest
  joins, leaves or is sent away, by id alone.

### Fixed

- A seek a player made to keep in step that took longer than a moment to finish was sent to
  the party as the viewer's own, moving everyone to that player's place, or leaving a party
  that waited for someone paused for good. A player stepping over a gap in its stream no
  longer moves the party either.
- A party no longer pauses for a player whose stall comes from its own catch-up seek.
- The player a party waited for is no longer moved on to the party's place when it's ready,
  which made it load afresh and the party wait for it again; it plays on from where it is and
  catches up by speed.
- A guest who leaves no longer has their player's pause sent as they go, which could replace
  "You left the watch party" with the party-ended message.
