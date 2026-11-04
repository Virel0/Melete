### Added

- **A cover palette:** an album's `colours` now carry `palette`, the cover's main colours (up
  to five, the most of it first), for what's around a square cover on a screen of another
  shape, such as a full-screen player, rather than stretching it. Colours worked out before
  are worked out again once.

### Changed

- Motion covers are calmer: the camera stays still and breathes slowly in and back over 16
  seconds, rather than going round a circle over 8, and on covers with a clearly distant, soft
  part (a sky, a haze, a plain backdrop) that part alone drifts a little. Every motion cover
  is made again (the loop's version is 3), so the renderer image of this version is needed.
  Loops are about twice the size they were.
