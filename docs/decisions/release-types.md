Decision: An album's release type is its matched MusicBrainz release group's primary type
(Album, Single, EP…) and secondary types (Live, Compilation…), served on every album as
`primary_type` and `secondary_types` from what the match already stores; nothing more is fetched.
An artist's page groups their albums into Albums, Singles & EPs, Live and Compilations; an
album with no known type (not matched, or matching off) stays under Albums.

Why: The match already keeps the release group's types, so grouping costs no request and sends
nothing beyond what matching does; an album that can't be told apart is still an album.

Date: 2026-11-07
