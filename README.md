# Melete

[![CI](https://github.com/Virel0/Melete/actions/workflows/ci.yml/badge.svg)](https://github.com/Virel0/Melete/actions/workflows/ci.yml)
[![License: GPL-3.0-or-later](https://img.shields.io/badge/license-GPL--3.0--or--later-blue.svg)](LICENSE)

Melete is a home media server in one Rust program and one Docker container. It does the work
of Jellyfin, Sonarr, Radarr, Lidarr and Prowlarr: it serves a household's movies, series and
music, and finds, downloads and imports what the household asks for. Everything lives in one
SQLite database, so nothing has to guess what another app did.

It adopts an existing library where it is. Library folders are mounted read-only until
automation is turned on, and Melete never renames, moves or deletes what is already there. The
one exception, once automation is on: a file replaced by a confirmed upgrade goes to its
library's `.melete-trash` folder and is deleted 30 days later.

Finding and downloading are optional. Until an admin sets up where to search (indexers or
Soulseek) and how to download, Melete only serves the library that is already there.

The design and every decision behind it, with the reasons, are in
[`docs/DESIGN.md`](docs/DESIGN.md).

## Legal use

Melete manages a media library you own or have the right to use. It doesn't host, index or
provide any content, and it isn't affiliated with any indexer, tracker or other service it can
connect to. You are responsible for following the laws where you live: don't use it to download
or share material you don't have the rights to.

## Features

**Watching**
- Movies, series and anime libraries, adopted as they are: folder and file names are read,
  nothing is renamed. `.nfo` files, local artwork, extras and subtitle files beside a video are
  used.
- Strict matching to TMDB (an id in the folder name wins; otherwise exactly one result must
  fit), and matching by hand for the rest.
- Direct play with byte ranges, or HLS conversion with ffmpeg on the CPU or an NVIDIA GPU
  (NVENC, with tone mapping), falling back to the CPU when the GPU stops working.
- Subtitles as WebVTT or burned in, audio and subtitle language preferences per user, and
  optional subtitles from OpenSubtitles.
- Resume, Continue watching, Next up, My List, a calendar of upcoming episodes and releases,
  cast and collections, chapters, and optional intro and credit detection, seek-bar previews and
  offline copies for devices.

**Listening**
- Music libraries with tags read in-process, optional MusicBrainz matching, covers, cover
  colours, classical music by composer and work, lyrics (local `.lrc` files or LRCLIB), lyrics
  search, translation and pronunciation.
- Streaming with conversion for the device and gapless information, play history, likes,
  playlists and pins kept the same on every device, listening together, and ambient mixes.
- Audio analysis in the background (loudness, tempo, beat grid, key, arrangement) for the
  apps' Next and Discover, and an optional Auto DJ that mixes on the parts of each track, split
  by an optional separator container.

**Getting more**
- Wanted lists for movies, series, artists, albums and songs, imported from Radarr, Sonarr and
  Lidarr or added by hand, with quality profiles and custom formats imported from them and
  editable in Melete.
- Indexers imported from Prowlarr, any Torznab indexer added by hand, and built-in presets for
  public trackers. Every release decision is stored with its scoring breakdown, so "why did it
  grab this?" has an answer.
- Downloads through Deluge on a seedbox, copied home over SFTP with the host key pinned, and
  imported into the right folder; music also through Soulseek via slskd. RSS sync, upgrades,
  a blocklist, and import lists from TMDB, IMDb and Rotten Tomatoes.
- Requests: anyone signed in can ask for a movie, a series, an artist, an album or a song, and
  follow its progress; admins approve or let requests through at once.

**Running it**
- A web UI built into the binary: Watch, Listen and Manage. Plain HTML, CSS and JavaScript
  modules, with no outside requests and no build step.
- A documented API (`/api/v1`, OpenAPI at `/api/v1/openapi.json`) for the client apps.
- Users with passwords, per-device sessions, scoped API keys, Quick Connect, optional sign-in
  through your own OpenID Connect provider, and per-user libraries and age ratings.
- Ways in: what each network (home, a VPN, a reverse proxy) may reach, by the address
  connections come from. See [`docs/REMOTE-ACCESS.md`](docs/REMOTE-ACCESS.md).
- Daily database backups and a restore command.

## Client apps

Melete serves its own web UI, and two native apps are built against its API:

- **Mneme** for video.
- **Aoide** for music. Melete keeps Aoide's sync store (playlists, likes, play history) and its
  `/aoide/*` endpoints.

## Install with Docker Compose

You need Docker with the Compose plugin. Melete runs as uid 10001, or a user you choose (see
[Running as another user](#running-as-another-user)), with a read-only root filesystem and no
capabilities.

1. Clone this repository on the server, or check out a release's tag (`git checkout v0.1.0`).
2. Copy `.env.example` to `.env` next to `compose.yaml` and set the folders. Each must exist;
   compose stops rather than let Docker create a missing one as root.

   ```sh
   # The data folder: the database, caches and backups.
   MELETE_CONFIG_DIR=/path/to/melete/config
   # Converted segments and offline copies.
   MELETE_TRANSCODE_DIR=/path/to/melete/transcode
   MELETE_MOVIES_DIR=/path/to/movies
   MELETE_SHOWS_DIR=/path/to/shows
   MELETE_ANIME_DIR=/path/to/anime
   MELETE_MUSIC_DIR=/path/to/music
   ```

   The config and transcode folders must be writable by Melete's user, uid 10001 unless you
   choose another, and the libraries readable by it:

   ```sh
   sudo install -d -o 10001 -g 10001 /path/to/melete/config /path/to/melete/transcode
   ```

   Never commit `.env`: it can hold passwords and keys.

   `.env` holds only what must exist before Melete starts: the folders, keys and passwords, and
   connections such as the seedbox's, each optional and left empty in the example. The
   services' other settings, such as turning MusicBrainz or lyrics on, are listed commented out:
   they're usually set in the web UI, under Settings → Outside services. A value set in `.env`
   wins and can't be changed from the web UI, so uncomment one only to fix it for good.
3. Choose the image in `.env`; `compose.yaml` itself needn't change. It runs the release it
   names, `ghcr.io/virel0/melete:0.1.0`. To pin it by its digest (see
   [Checking a release](#checking-a-release)), or to run another version:

   ```sh
   MELETE_IMAGE=ghcr.io/virel0/melete:0.1.0@sha256:<digest>
   ```

   To build from this repository instead, for development, add `compose.build.yaml`; the
   first build takes a while, and `docker compose up -d --build` builds again after a change:

   ```sh
   COMPOSE_FILE=compose.yaml:compose.build.yaml
   ```
4. Without an NVIDIA GPU, remove the `deploy:` block from `compose.yaml` and set
   `MELETE_HWACCEL: none` (the CPU). With one, the host needs the NVIDIA Container Toolkit.
5. Libraries are set in `compose.yaml` as `MELETE_LIBRARY_<NAME>=<kind>[,<category>]:<path>`,
   where the kind is `movies`, `shows` or `music` and the category `anime` or `kids`. Change
   them to match your folders.
6. Start it:

   ```sh
   docker compose up -d
   docker compose logs -f melete   # wait for "scan finished"
   ```

The web UI is at `http://<server>:8100/`.

### Updating

Read the release's notes, then name the new version in `MELETE_IMAGE` (or, when it's unset,
update this repository's checkout: its `compose.yaml` names the newest release), and:

```sh
docker compose pull
docker compose up -d
```

Melete backs up its database before a new version changes it (see
[Backups and restore](#backups-and-restore)).

### First run

Create the first admin. The command prints a generated password once:

```sh
docker exec melete melete user add alice --admin
```

Sign in and change the password under Settings. More users can be added from the web UI or
with `melete user add <name>`; `melete user reset-password <name>` sets a new one and signs
that user out everywhere.

Outside services (TMDB for metadata, indexers, Deluge and so on) are off until you set them
up, in `.env` or under Settings → Outside services. A value set in `.env` wins and can't be
changed from the web UI.

### Optional containers

Each is added through `COMPOSE_FILE` in `.env`, for example
`COMPOSE_FILE=compose.yaml:compose.soulseek.yaml`, and each needs the folders `.env.example`
lists for it:

- `compose.soulseek.yaml`: slskd behind a Gluetun VPN, for Soulseek searches and downloads.
  Melete's key for slskd's API needs the ReadWrite role and no more (`role=ReadWrite`, as this
  file sets it): enough to search and download, not to administer slskd. If you run slskd
  some other way, give Melete such a key.
- `compose.stems.yaml`: the stem separator (needs an NVIDIA GPU), for Auto DJ and sing-along.
- `compose.motion.yaml`: the motion cover renderer.

The separator and the renderer are built from this repository (`stems/`, `motion/`). Their
models are downloaded from their publishers on first start, never shipped.

### Running as another user

Melete runs as uid and gid 10001, its image's own user, unless `MELETE_USER` in `.env` names
another. Many libraries already belong to a user of their own, often uid 1000; instead of
changing the ownership of a large or network-mounted library, run Melete as that user:

```sh
MELETE_USER=1000:1000
```

Nothing in the image depends on uid 10001: it has no home folder to write to, and writes only to
the folders compose mounts. Melete's user, whichever it is, needs:

- to read the libraries (read on files, read and execute on folders), and to write to them once
  automation is on;
- to read and write the config and transcode folders;
- to read and write the folders of the optional containers (`.env.example` lists them). The
  stem separator, the motion cover renderer and slskd run as the same user, so each can read
  what the other writes.

A share whose top folder gives nothing to others (mode `770`, or anything without read and
execute for "other") can't be read at all by a user outside its owner and group. The fix is to
run Melete as that owner, or as a user in that group (`MELETE_USER=10001:<the share's gid>`),
not to change the library's permissions.

### Turning on automation

Library mounts are read-only (`:ro`) in `compose.yaml`. Melete can search and decide with them
read-only, but importing needs write access: drop `:ro` from a library's mount and let
Melete's user write to that folder on the host. Until then, finished downloads wait on the
seedbox.

## Backups and restore

Melete copies its database with SQLite itself, so a copy taken while it runs is whole. Copies
go in the data folder's `backups/`:

- one before any migration runs (startup stops if it can't be made); the last 5 are kept,
- one a day: the last 7 days' are kept, and one a week for 4 weeks before them,
- one whenever you run `docker exec melete melete backup`, kept until you remove it.

To go back to a copy, stop Melete first:

```sh
docker compose stop melete
docker compose run --rm melete melete restore            # lists the copies, newest first
docker compose run --rm melete melete restore <copy>     # checks it, keeps the current database, then replaces it
docker compose start melete
```

The backups hold everything in the database. The secrets in it (keys and passwords of outside
services set from the web UI, indexers' API keys, two-factor sign-in, and search results'
download links, which can carry an indexer's key or a tracker's passkey) are sealed with the
key in the data folder's `secrets.key`, which Melete makes at its first start and no backup
holds. Without that file, a backup gives none of them back:

- Back up `secrets.key` apart from the database copies, somewhere only you can read, and keep
  it for as long as you keep the copies.
- When Melete starts and the key is missing, or isn't the one the secrets were sealed with,
  it stops and says which secrets it can't read. Put the key back; or, with Melete stopped, run
  `docker compose run --rm melete melete secrets clear` to clear the services' secrets,
  indexers' keys, two-factor setups not yet confirmed and search results' links (then enter
  the secrets and keys again in Settings, and search again), and
  `docker compose run --rm melete melete user reset-password <name> --remove-second-factor`
  for each user whose two-factor sign-in it names.

On the first start of a version that seals secrets, Melete seals the ones the database kept
in plain text, then rewrites the database (`VACUUM`) and empties its write-ahead log, so the
old bytes don't linger in freed pages. It then seals the secrets in each copy already in
`backups/`, in place: the copy made just before that upgrade and the daily and weekly ones
before it. A copy it can't open is named in a warning, kept as it is, and tried again at the
next start. Copies of the data folder you made elsewhere before the upgrade still hold the
secrets in plain text: delete them, or keep them as safe as the server.

Sealing keeps the secrets from someone who can read the database or a backup without the key
file. It doesn't stop someone who can change the database. A setting's secret is bound to its
name and a two-factor secret to its user, so neither opens if moved to another; an indexer's
key, a download link and a guid are bound only to their column.

Secrets set in `.env` stay in `.env`, not in the database.

## Privacy

Melete sends nothing anywhere until you set a service up, and it never sends telemetry,
analytics or crash reports. Requests to outside services carry only what each service needs to
answer: no names, emails, account or repository links, and a User-Agent that names only Melete
and its version. Every service below is off by default; Settings → Outside services shows each one
with what leaves the server.

| Service | What it's for | What leaves the server |
|---|---|---|
| TMDB | Titles, posters, episode lists, ratings, cast | Titles to look up, TMDB ids, the metadata language, and the image files to fetch from its image server |
| TMDB trending and similar titles | Discovery rows, franchise gaps | Which chart, or a title's or collection's TMDB id; optionally posters of titles not in the library, fetched once and cached |
| Trailers online | Trailers for titles without one | The server asks TMDB for a title's videos by its id. YouTube (youtube-nocookie.com) is loaded by the viewer's own device, only when they press play |
| MusicBrainz and the Cover Art Archive | Matching music, covers, music requests | Album and track names, the words of a music request search, and a release group's id for a cover |
| LRCLIB | Synced lyrics | Each track's title, artist, album and length, once per track |
| LibreTranslate | Translating lyrics | The lines of each song translated, to the server you set (you can run your own) |
| OpenSubtitles | Missing subtitles | A video's OpenSubtitles hash (from its size and first and last 64 KB), its TMDB or IMDb id, season and episode, and the language; nothing about who asked |
| OpenRouter (AI) | Finding titles by describing them | The words typed for that search, with the request that providers not keep or train on it |
| OpenID Connect | Signing in through your own identity provider | Melete's client id and secret and the sign-in code; only the `openid` scope is asked for |
| Indexers (Prowlarr feeds, Torznab, built-in public-tracker presets) | Searching for releases | The title, ids, season and episode searched for, and periodic RSS requests |
| FlareSolverr | Trackers behind Cloudflare's browser check | The tracker page to fetch, to your FlareSolverr |
| Deluge on a seedbox, and its SFTP | Downloading and copying home | Torrents to grab; SFTP signs in with a pinned host key, so the password goes to no other server |
| slskd (Soulseek) | Music searches and downloads | Search words (artist and album, or a song), sent by slskd to the Soulseek network under its account. Melete never connects to peers |
| Import lists (TMDB, IMDb, Rotten Tomatoes) | Adding titles to the wanted list | Which chart or list to read |
| Radarr, Sonarr, Lidarr, Prowlarr imports | Bringing over lists, profiles and indexers | Run by an admin; the API key given is used for that import (Prowlarr's is kept for its feeds) |

Peer-to-peer traffic happens only on the seedbox or in slskd. The web UI loads nothing from
outside Melete, except a YouTube trailer a viewer chooses to play when trailers are on.

**What Melete stores**, all in its data folder (`/config`) and transcode folder (`/transcode`):

- The SQLite database: users (passwords hashed with Argon2id; session tokens and API keys stored
  only as SHA-256 hashes), watch progress, play history, likes, playlists, requests, the wanted
  lists, download history, release decisions, diagnostics reports apps send about themselves
  (anything in them that looks like a secret is removed before it's kept), settings, and the
  keys and passwords of outside services set from the web UI, indexers' API keys, two-factor
  secrets and search results' download links, sealed (AES-256-GCM) with the key in
  `secrets.key`.
- `secrets.key`: the key those secrets are sealed with, readable by Melete's user alone.
- Caches: artwork, fetched subtitles, seek-bar previews and motion covers.
- Database backups in `backups/`.
- Converted segments and offline copies in the transcode folder. An offline copy is deleted 7
  days after its last download, or when the person who asked deletes it.

Melete writes to the libraries only once automation is on, and then only what it imports
(files, tags, covers and `.lrc` lyrics it placed) and the trash of replaced files.

## Checking a release

A release is a container image on GitHub's container registry, built by this repository's CI
from a `v*` tag, never on a personal machine. Each release page gives the image's digest and
carries the same image as an OCI archive, with its SHA-256 in `SHA256SUMS`. The image has a
signed build-provenance attestation.

Pull by digest, so the image can't change under you:

```sh
docker pull ghcr.io/virel0/melete:0.1.0@sha256:<digest>
```

Check that the image was built by this repository's release workflow (needs the GitHub CLI):

```sh
gh attestation verify oci://ghcr.io/virel0/melete:0.1.0@sha256:<digest> --owner Virel0
```

To use the archive instead, check it against the release's checksums, then load it:

```sh
sha256sum --check SHA256SUMS
docker load --input melete-0.1.0.oci.tar
```

When the release has a `SHA256SUMS.asc`, the checksums are signed with the project's release
key: `gpg --verify SHA256SUMS.asc SHA256SUMS`.

## Contributing and security

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for the layout, the checks and the rules. Report
security problems privately, as [`SECURITY.md`](SECURITY.md) explains.

## License

GPL-3.0-or-later. See [`LICENSE`](LICENSE).
