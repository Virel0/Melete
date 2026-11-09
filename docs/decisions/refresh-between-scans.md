Decision: Besides the refresh after each scan, Melete looks every hour for series due for a TMDB refresh and fetches only those: library series whose cached entry is stale or that have an episode TMDB hasn't filled in yet, and monitored wanted series still airing or with such an episode, by the same rules (7 days, daily, 6 hours). It doesn't scan libraries, match new folders or fetch anything else; it never runs at the same time as the scan's refresh. Only the series' TMDB ids leave the server.

Why: Refreshes only ran after a scan, so a schedule longer than 6 hours (a daily scan, say) held back new episodes' titles and the daily refresh of airing series until the next scan. A scan wakes every disk, so the refresh runs on its own instead of asking for one.

Date: 2026-11-08
