Decision: When TMDB answers that a matched movie or series is gone, Melete notes when (`gone_at`, migration 0200), keeps serving the cached copy, logs it, and asks TMDB again a day later rather than at every refresh run. The title is listed to be matched again (status `gone` in `GET /api/v1/metadata/unmatched`, counted in the titles_unmatched health note), even when it was matched by hand, until TMDB has the entry again (storing it clears the mark) or an admin matches it to another one. A wanted series TMDB no longer has is asked about once a day the same way.

Why: The stale entry was fetched again at every run, because nothing was stored when TMDB had nothing, so the same line was logged every hour and nobody was told the match needed fixing. A vanished entry usually means TMDB merged or removed it, which only a new match fixes, so it belongs in the list where matches are made. A hand-made match is listed too, since the entry it chose is no answer any more.

Date: 2026-11-28
