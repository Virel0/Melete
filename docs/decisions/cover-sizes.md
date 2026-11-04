Decision: An album cover can be asked for at 160, 400, 600 or 800 pixels wide. Melete makes it
once with ffmpeg as a JPEG from the cover the album has now (its folder's, its tags' or the
Cover Art Archive's), keeps it beside the cached covers under a name that changes with the
cover, never enlarges it, and makes at most four at once. Without a size the cover is served as
it is.

Why: Music apps draw covers from 28-point rows to full-width players, and every place got the
whole cover, often a large folder image. Four sizes cover the places apps draw at; a limit on
resizing at once keeps a grid opening from starting hundreds of ffmpeg runs.

Date: 2026-11-04
