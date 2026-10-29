# Security

## Reporting a vulnerability

Report it privately through GitHub's private vulnerability reporting: on this repository, open
**Security → Report a vulnerability**. Please don't open a public issue, pull request or
discussion about it.

Say what is affected, how to reproduce it, and what an attacker gains. Leave out real
passwords, keys, addresses and media paths; made-up values are enough. The report stays private
between you and the maintainer until a fix is released.

Fixes go into the next release. Only the latest release is supported.

## What Melete can access

Melete runs in one container as uid 10001, with a read-only root filesystem, every Linux
capability dropped and `no-new-privileges` set (`compose.yaml`). It is given no Docker socket
and no host network. What it can reach is what the compose files mount and what an admin sets
up:

**Files**
- **Library folders** (`/media/*`): mounted read-only until an admin turns automation on by
  removing `:ro`. Melete reads them to scan and play, and runs `ffprobe` and `ffmpeg` on their
  files. With write access, it writes only what it imports (files, tags, covers and `.lrc`
  lyrics it placed) and moves a file replaced by a confirmed upgrade into that library's
  `.melete-trash`, deleting it 30 days later. It never renames, moves or deletes anything else.
- **The data folder** (`/config`): read and write. The SQLite database (users, hashed passwords
  and tokens, history, settings, and the keys and passwords of outside services set from the
  web UI), caches, and database backups.
- **The transcode folder** (`/transcode`): read and write. Converted segments and offline
  copies.
- **Download folders**, only when set up: the slskd downloads folder (`compose.soulseek.yaml`),
  where Melete deletes its own finished downloads after importing them; and the job folders it
  shares with the optional stem separator and motion cover renderer (`compose.stems.yaml`,
  `compose.motion.yaml`).
- **A GPU**, when `compose.yaml` reserves one for conversions.

**Network**
- It listens on port 8100 for the web UI and the API. What each network may reach can be
  limited by address (`MELETE_WAYS_IN`; see `docs/REMOTE-ACCESS.md`).
- It contacts outside services only once each is set up, and each is off by default: TMDB,
  MusicBrainz and the Cover Art Archive, LRCLIB, LibreTranslate, OpenSubtitles, OpenRouter, an
  OpenID Connect provider, indexers (Prowlarr feeds, Torznab and built-in public-tracker
  presets), FlareSolverr, import lists (TMDB, IMDb, Rotten Tomatoes), Deluge, the seedbox over
  SFTP, slskd, and Radarr, Sonarr, Lidarr and Prowlarr when an admin imports from them. The
  README's privacy section says what each one is sent.
- On the seedbox, SFTP signs in only to the pinned host key, and reaches only the folder
  mapped in `MELETE_SEEDBOX_PATHS`. Melete deletes data there only when an admin removes a
  download and asks for its data to be deleted too.
- It never connects to peer-to-peer networks itself: that happens only on the seedbox (Deluge)
  or in slskd.

**Secrets** (indexer and service keys, the seedbox and Deluge passwords) stay on the server.
They are never put in API responses, logs, URLs or error messages, and secrets set from the
web UI can be replaced but not read back.
