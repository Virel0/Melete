### Changed

- A title's own poster and backdrop (`poster.*`, `fanart.*` in its folder) are cropped to the
  shape of their kind, 2:3 and 16:9, around the picture's subject, so an image of another shape
  never comes out stretched or letterboxed. They get new artwork URLs at the next scan, so apps
  fetch them again.
