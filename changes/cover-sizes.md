### Added

- Album covers at the size an app draws them: `/api/v1/music/albums/{id}/cover?size=160`
  (or 400, 600, 800), a JPEG made once from the cover and kept, never larger than the cover.
  A song row no longer downloads a large folder image.
