Decision: A movie search turns down a release whose name marks a frame rate converted up from
the film's own: 48, 50, 60 or 120 fps (with or without a separator), or "interpolated". The
reason is kept with the decision as any other. An admin can allow them
(`MELETE_ALLOW_HFR_MOVIES`, off by default). Films really made at a high frame rate are never
turned down for it (by TMDB id: Billy Lynn's Long Halftime Walk, Gemini Man and the three Hobbit
films), and series are left alone, where 50 and 59.94 fps are normal.

Why: A film shot at 24 fps and made up to 60 has invented frames, and some players, Apple's among
them, can't keep up with them; such a release played at a fraction of its frames on one while
other films played smoothly. These releases say what they are in their names, so turning them
down at search time keeps them out without probing anything.

Date: 2026-11-04
