### Changed

- Watch parties keep in step more closely. Each player sets its clock by the server's,
  NTP-style (the answer with the shortest round trip among the latest few), checks itself
  every second rather than only when the party changes, and closes a small drift by playing a
  little faster or slower (0.95× to 1.05×) instead of jumping; past 1.5 seconds (2.5 for a
  conversion, whose seeks are coarser) it still seeks. A party set playing starts 0.4 seconds
  ahead, so everyone starts at the same moment rather than as the news reaches each. A player
  the browser won't start by itself says "Press play to join in", and pressing play catches up
  with the party.
