Decision: The films high frame rate releases are wanted for are a list admins keep, by TMDB id
(`high_frame_rate_films`), switched from each movie's Manage page; it starts with the films
really made at a high frame rate, in place of a fixed list in the code. A movie whose present
files' highest probed frame rate (their first video stream's) is 48 fps or more is noted:
`high_frame_rate` on the movie's answer (`{fps, allowed}`, null otherwise), a note on its page,
and, unless its film is on the list, a `movies_high_frame_rate` note in health's Movies area.
For a film not on the list the note says it's most likely made up from 24, with a link for
admins to search for another release; for one on it, only that it may stutter. Series aren't
looked at.

Why: Which films are really made at a high frame rate keeps growing and isn't Melete's to
decide film by film in code; by TMDB id the choice holds before a film is in the library. A
release turned down by its name can still arrive under a name that doesn't say so, and films
imported before the check are already there; the frame rate Melete already probes says it
plainly.

Date: 2026-11-04
