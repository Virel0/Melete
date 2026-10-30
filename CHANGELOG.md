# Changelog

Every notable change to Melete is listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). The reasons behind each behaviour
are in the decisions table of [`docs/DESIGN.md`](docs/DESIGN.md).

## [Unreleased]

### Added

- Albums can be liked. An album's like is a sync record of its own (`album_likes`), so it
  reaches Aoide's apps the way a song's does. The web has Like on an album's page and a Liked
  albums page.

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
