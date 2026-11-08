Decision: A series (by its TMDB id) follows TMDB's own episode order or one of its TMDB episode
groups, kept in `series_orders` with who chose it (`auto` or `admin`) and when. Everything that
numbers episodes asks one view, `episodes_in_order`: browsing and the apps' series and season
routes, Missing and Monitored, the calendar, Next Up and Continue, searches and RSS sync,
decision 34's absolute numbers (counted through the order followed) and the running-time check
on import. A group's parts in TMDB's order are seasons 1, 2, ... (a part called
Specials is season 0), each part's episodes numbered from 1; titles, air dates and stills stay
each episode's TMDB data. Specials the group leaves out stay season 0 as TMDB numbers them,
unless the group has specials of its own. Groups are asked of TMDB (`/tv/{id}/episode_groups`,
then `/tv/episode_group/{id}` for each, at most 20) only for a matched series whose files use
seasons TMDB's own order doesn't have, or that follows a group, and again after each refresh of
its metadata, through the TMDB client (its timeouts and pace); a group TMDB stops listing is
forgotten unless the series follows it. Melete switches a series by itself only when nobody
chose its order, its files (by name; numbers worked out from absolute ones and specials left
out) use a season TMDB's own order doesn't have, and exactly one group has exactly the files'
seasons and episodes; otherwise the group holding most of the files' episodes, more than TMDB's
own order does, is suggested. An admin's choice, a group or TMDB's own order, is final. A switch
renumbers, in the same transaction and through each episode's TMDB id, what's kept by season
and episode number: monitoring (a season of the new order is monitored when any of its episodes
was, and those that weren't are left out on their own), downloads, searches and history that
name an episode; those naming only a season are left as they are, and so are numbers the old
order didn't have. Seasons the new order doesn't have stop being monitored. What names an episode
the new order leaves out loses its season and episode, which would be another episode's, and
keeps the episode's TMDB id (`left_out_tmdb_id`); an order that has the episode again numbers it
again. A switch waits (nothing changes, and Melete tries again next run) while a download is on
its way for such an episode, or a season pack for a season the new order makes up of other
episodes. A series' search result is grabbed only when it was searched for after the series
last changed order (searches are kept as of when they started), so nothing judged in another
order is grabbed. Files keep their names and ids, so watch progress and watched state, kept per
file, stay with them; files numbered from the first episode are counted again with the switch,
and one whose number has no place in the new order loses it. A swap from Make room searches for
its file's episode as numbered when it runs. A season asked for by TMDB's number (a request)
is, for a series following a group, the group's seasons holding that season's
episodes: those are listed, monitored and searched for, its other episodes there left out on
their own. It's recorded in the series' history (`reordered`). What leaves the server: the
series' TMDB id and the groups'
ids, to TMDB, which already has them; it stops with MELETE_TMDB_TOKEN unset, when cached groups
still serve.

Why: TMDB lists some anime as one long season while files (and other sites) split it, so the
later seasons' files showed as Untitled episodes, TMDB's later episodes as Missing and
Monitored, and an absolute-numbered release mapped through TMDB's order could be grabbed again
as a duplicate. Keeping one order per series that everything asks keeps browse, search, RSS and
import agreeing, as decision 34 wanted. Switching only on an exact fit fails closed: a near fit
is a guess an admin confirms. Numbers are renumbered through TMDB's episode ids because those
are the same episode whatever the order.

Date: 2026-11-07
