Decision: A matched series, in the library or wanted, with an episode aired within the last
14 days or airing within a day that has no name, TMDB's placeholder name or no summary, is
fetched again when its entry is over 6 hours old, until TMDB fills the episode in or the 14
days are up. The placeholder is the word TMDB uses in the metadata language (or English)
followed by that episode's own number, so a real title such as "Episode Zero" isn't taken for
one. A file first found by a scan after its series was fetched, for an episode still without
a name or with the placeholder, has the series fetched again on the refresh after that scan.
Each scan records when it first found a file, for this. Only the series' TMDB id is sent, as
with every refresh.

Why: TMDB lists a new episode under a placeholder name and fills in the title, summary and
still later, so a weekly (or, for wanted series, daily) refresh left new episodes untitled for
days. Asking sooner only for series in that state keeps the extra requests to a few series at
a time.

Date: 2026-11-07
