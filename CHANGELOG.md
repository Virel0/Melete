# Changelog

Every notable change to Melete is listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). The reasons behind each behaviour
are in the decisions table of [`docs/DESIGN.md`](docs/DESIGN.md).

## [Unreleased]

### Added

- Ctrl+K search finds artists and songs. A song opens its album with the song picked out.
- `q_in=track` on `GET /api/v1/music/tracks` matches a track's own title and artist only.

### Fixed

- The web player no longer shows "null" beside Full screen for a movie or an episode without a
  neighbour, nor does the music queue, the AI search or a Soulseek download's Remove.
- Double-clicking the picture takes the player full screen, and the video's own full-screen
  request hands over to the player's stage, so Skip and the next episode come along.
- Manage's library pages show each title's own poster, as Watch does, and an album cover for
  each artist, instead of empty boxes.
- An artist with some albums tagged with MusicBrainz ids and some not is listed once. Their old
  address and pins lead to the artist they joined.
- Matching moved and renamed files after a scan finds them by the movie and episode indexes,
  rather than comparing each new file with every file in its library.
- An album tagged with a MusicBrainz release that has since been merged into another matches
  the surviving release, instead of every lookup failing with "answered 301".

## [0.1.0] - 2026-10-29

The first release.

### Added

**Libraries and metadata**
- Movies, series (with an anime or kids category) and music libraries, adopted in place from
  read-only mounts: nothing renamed, moved or deleted. Scans on a schedule, after imports, on
  request, and optionally when a folder changes.
- Strict TMDB matching, matching by hand, metadata in a chosen language, textless backdrops,
  logos, cast, crew and collections, age ratings by a chosen country's system.
- `.nfo` files, local artwork, extras and renamed folders read as they are.
- Music tags read in-process, optional MusicBrainz matching, Cover Art Archive covers, cover
  colours, multiple artists per track, classical music by composer and work, Spatial Audio files.

**Playing**
- Direct streams with byte ranges, and HLS conversion with ffmpeg on the CPU or an NVIDIA GPU,
  with a fallback to the CPU and a limit on conversions running at once.
- Subtitles as WebVTT or burned in, subtitles that aren't UTF-8 converted, optional subtitles
  from OpenSubtitles, and per-user audio and subtitle languages.
- Watch progress, Continue watching, Next up, My List, a calendar, chapters, optional intro and
  credit detection, seek-bar previews, offline copies and trailers.
- Music streaming with conversion for the device, gapless information, play history, likes,
  playlists, pins, a Home view, ambient mixes, listening together, and what the household is
  listening to (each user's choice).
- Lyrics from `.lrc` files or LRCLIB, lyrics search, translation through LibreTranslate, and
  pronunciation in Latin letters.
- Audio analysis in the background (bounds, loudness, tempo, beat grid, key, arrangement), Next
  and Discover for the music app, and an optional Auto DJ using an optional stem separator.
- Optional motion covers made from albums' still covers.

**Getting more**
- Wanted lists for movies, series, artists, albums and songs, imported from Radarr, Sonarr and
  Lidarr or added by hand, with monitoring by season and episode.
- Quality profiles and custom formats, imported and editable, and music quality profiles.
- Indexers imported from Prowlarr, added by hand as Torznab, or built in as presets; searches
  with every decision stored and shown; RSS sync, upgrades, cutoff unmet, Search all missing, a
  blocklist, and import lists from TMDB, IMDb and Rotten Tomatoes.
- Downloads through Deluge on a seedbox, copied home over SFTP with a pinned host key and
  imported into the right folder; replaced files go to a trash for 30 days. Soulseek through
  slskd for albums and songs. Playlist imports as song requests.
- Requests for everyone signed in, with approval, progress of each title, and a history of each
  title.

**Web UI and API**
- A web UI built into the binary, in three sides: Watch, Listen and Manage.
- The `/api/v1` API with its OpenAPI contract, Aoide's `/aoide/*` sync endpoints, and live
  updates as Server-Sent Events.
- Accounts with Argon2id passwords, per-device sessions, scoped API keys, Quick Connect,
  optional OpenID Connect sign-in, per-user libraries and highest age rating.
- Ways in: what each network may reach, by the address connections come from, with a bitrate
  limit per way in.
- Outside services set up and tested from the web UI, each off by default and saying what
  leaves the server.
- Admin status: health of every service, disk space, diagnostics reports from apps, and a speed
  test.

**Operations**
- One Docker image with ffmpeg, a read-only root filesystem and no capabilities; optional
  compose files for slskd behind a VPN, the stem separator and the motion cover renderer.
- Database backups before migrations, daily and by hand, and `melete restore`.

<!--
Compare links. After the first release, the pattern is:
[Unreleased]: https://github.com/Virel0/Melete/compare/vX.Y.Z...HEAD
[X.Y.Z]: https://github.com/Virel0/Melete/compare/vW.V.U...vX.Y.Z
and for the first release:
[0.1.0]: https://github.com/Virel0/Melete/releases/tag/v0.1.0
-->
[Unreleased]: https://github.com/Virel0/Melete/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/Virel0/Melete/releases/tag/v0.1.0
