Decision: The web UI's search page searches the library, and sends the words typed to TMDB for
titles not in the library only while trending and similar titles are on (`MELETE_DISCOVERY`,
with TMDB set up), as `GET /api/v1/discover/status` says. With them off it shows only the
library's matches. Asking for something by name stays on the Requests page, which searches TMDB
when asked to whatever the setting.

Why: The palette leads to the search page, so a search meant for the library would otherwise
send its words out by default; a household that left discovery off keeps library searches on
the server.

Date: 2026-11-06
