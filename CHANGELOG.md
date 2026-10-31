# Changelog

Every notable change to Melete is listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). The reasons behind each behaviour
are in the decisions table of [`docs/DESIGN.md`](docs/DESIGN.md).

## [Unreleased]

## [0.3.1] - 2026-10-31

### Fixed

- Soundtracks and other compilations weren't found on Soulseek: every word of "Various Artists"
  and the album's bracketed part had to be in a folder's name. When Soulseek answers nothing,
  it's now asked again without them, and a compilation's folder may name no artist as long as
  its whole title is there. Songs get the same second try.

## [0.3.0] - 2026-10-31

### Upgrading from 0.2.0

Nothing to do: no database changes, no new settings. Apps that ask for converted video keep
getting what they got before unless they use the new fields below.

### Added

- **Copy mode keeps what the player can take.** A transcode with `video_codec: copy` can list the
  audio codecs the player takes (`audio_codecs`: `aac`, `ac3`, `eac3`, `flac`, `alac`) and whether
  it shows Dolby Vision (`dolby_vision`). Audio it takes is copied unchanged, so E-AC-3 keeps its
  Atmos. Lossless audio it can't take (TrueHD, DTS-HD MA, PCM) becomes FLAC or ALAC instead of AAC,
  and Dolby Vision profile 8 is labelled as Dolby Vision instead of HDR10.
- **Playlist pictures from the web and the API.** A playlist's owner sets or removes its picture on
  the playlist's page (Picture, beside Rename and Delete), or with `PUT` and `DELETE` on
  `/api/v1/playlists/{id}/image`. It reaches the Aoide apps through sync, as a rename does.

### Changed

- **The Watch home's hero plays previews.** A pick plays its trailer when its folder has one (converted
  when the browser can't play it as it is), and otherwise 40 seconds of the title itself, muted,
  from where you left off or a little way in. Conversions are only asked for on the home network,
  and a preview never counts as watching. The hero no longer uses YouTube. The setting is now
  called "Play previews on the Watch home".
- **Going back plays at once.** Up to ten minutes (at most a gigabyte) of converted video stays
  behind where you are, so seeking back doesn't restart the conversion. Copy mode also works
  further ahead, so skipping forward within ten minutes finds its part ready.
- A download whose files are copied and being checked against its title says "Checking it's the
  right title" instead of still showing the copy's progress.

### Fixed

- The hero showed YouTube's player error instead of a trailer: YouTube's player won't play
  without a referrer, which Melete's pages never send.
- Dolby Vision profile 5 copied as it is was labelled as Dolby Vision without the configuration
  record players need, in conversions and offline copies.

## [0.2.0] - 2026-10-31

### Upgrading from 0.1.0

- **Back up `secrets.key`.** On its first start 0.2.0 creates `secrets.key` in the data folder
  and seals every stored secret with it (API keys, passwords, two-factor secrets, links that can
  carry a tracker passkey), including inside the existing backup copies. Backups copy only the
  database, so keep a copy of `secrets.key` somewhere safe apart from them: without it Melete
  won't start, and the way back is putting the key back or clearing and re-entering those
  secrets (`melete secrets clear`).
- **GPU transcoding needs `compose.nvidia.yaml`.** `compose.yaml` no longer reserves a GPU. Add
  `compose.nvidia.yaml` to `COMPOSE_FILE` in `.env`, or Melete transcodes on the CPU.
- **The image comes from `.env`.** `compose.yaml` runs `${MELETE_IMAGE:-ghcr.io/virel0/melete:0.2.0}`;
  building from source is `compose.build.yaml`. Undo any local edit of `compose.yaml` before
  pulling, and set what you need in `.env` or `compose.local.yaml` instead.
- **Settings in `.env.example` are now commented out.** A value in `.env` locks a setting in the
  web UI; remove lines such as `MELETE_MUSICBRAINZ` or `MELETE_LYRICS` from `.env` to manage them
  in Settings.
- `MELETE_USER` runs the container as another uid (for example `1000:1000`, to match an existing
  library's owner).

### Added

- **Two-factor sign-in** with an authenticator app (TOTP), optional for every user, set up from
  Your account with a QR code. Quick Connect, OpenID Connect and API keys are unaffected;
  `melete user reset-password` can remove it.
- **The Watch home:** "Because you watched" rows, AI picks ("Featured for you", off by default,
  through the existing OpenRouter key), and the hero playing a pick's trailer. "More like this"
  on a title's page. Auto-play of the next episode and its countdown as a per-user setting.
- **Never importing a fake:** a download's running time is checked against TMDB before anything
  is placed, a film's releases wait until it can be out in that form (a minimum availability per
  profile), and an admin can set aside a wrong file Melete imported ("Wrong file?", to the trash).
  After three failed checks for a title, its automatic searches stop and health says so.
- **Manage:** Monitor on a title that's only in the library, Select all with bulk monitoring, live
  word-based search in Add new (with songs, and a Soulseek fallback for music), separate quality
  profiles for Series and Anime, TMDB matching as its own task with health findings.
- **Downloads:** copying progress ("Copying home: 1.2 of 4.3 GB"), an option to remove imported
  torrents from Deluge, imported titles matched at once instead of showing up grey, and requests
  given their service's only profile (or a health note asking for one).
- Seek-bar previews at 320, 480 or 640 pixels wide.
- Progress lines for long background jobs, and the files a scan couldn't parse, named once each.
- Albums can be liked. An album's like is a sync record of its own (`album_likes`), so it
  reaches Aoide's apps the way a song's does. The web has Like on an album's page, a heart on
  every album card in Listen, and a Liked albums page. A liked album stays liked when a rescan
  gives it a new id, as on the devices.
- Ctrl+K search finds artists and songs. A song opens its album with the song picked out.
- `q_in=track` on `GET /api/v1/music/tracks` matches a track's own title and artist only.

### Changed

- Watch lists Movies, Shows, Anime, in its sidebar and on the home.
- Settings: what needs TMDB sits under TMDB, a setting locked by `.env` names its variable and the
  way out, and only real outside services offer a Test.
- The default log leaves out the tag reader's per-file warnings, with one summary line a scan.

### Fixed

- Signing in no longer fails with "database is locked" during a large first scan.
- Soulseek works with a ReadWrite key; a Torznab feed without a key is asked without one, and a
  refusal is blamed on the key only when the feed says so; the FlareSolverr check accepts
  compatible solvers such as Byparr.
- A title taken off the list can be asked for again.
- The Aoide import no longer stops at a playlist that was exported to Jellyfin.
- Settings fit a phone's width.
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

### Security

- Stored secrets are sealed at rest (AES-256-GCM) with a key kept outside the database.
- The SFTP library (russh) is updated to 0.65 for its advisories; none of them was reachable from
  Melete, which only signs in to the pinned seedbox with a password.

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
[Unreleased]: https://github.com/Virel0/Melete/compare/v0.3.1...HEAD
[0.3.1]: https://github.com/Virel0/Melete/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/Virel0/Melete/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/Virel0/Melete/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/Virel0/Melete/releases/tag/v0.1.0
