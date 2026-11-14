Decision: Amends the season-episode-by-episode decision. Search on a title's page, which grabs the best accepted result itself, now asks the server to take the season's search as a sync would (`POST /api/v1/searches/{id}/take`, `rss::grab_for`) when no result is accepted but some are refused only for being single episodes of the season; the single episodes are then taken for its missing episodes as they are for an approved request or Search all missing. The route takes only a whole season's search.

Why: The title page's Search never reached the server's grabbing, so a season found only as single episodes was never taken from there, though its results said it would be.

Date: 2026-11-13
