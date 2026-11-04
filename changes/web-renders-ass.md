### Changed

- In a browser that can't draw ASS/SSA itself, choosing an ASS/SSA track converts the video so
  the server can draw a styled one into the picture (`renders_ass: false`), instead of showing
  only its words. Browsers that draw ASS ask for nothing new, so changing subtitles doesn't
  restart their conversion.
