### Added

- **Tall motion covers** for full-screen players: beside each square motion cover, a 9:16 loop
  (1080 by 1920) with the cover large and sharp over a blurred copy of itself that drifts
  slowly. Covers too flat or too full of text for a square loop still get one. Albums give it
  as `motion_cover_tall`, served at `/api/v1/music/albums/{id}/motion/tall`, shown as motion
  covers are.
- **A cover palette:** an album's `colours` now carry `palette`, the cover's main colours (up
  to five, the most of it first), for a gradient behind a square cover on a screen of another
  shape. Colours worked out before are worked out again once.

### Changed

- Motion covers move differently: the camera drifts across the cover, and slowly in and out,
  rather than going round a circle. Every motion cover is made again (the loop's version is 3),
  so the renderer image of this version is needed.
