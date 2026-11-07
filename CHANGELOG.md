# Changelog

Every notable change to Melete is listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). The reasons behind each behaviour
are in the decisions table of [`docs/DESIGN.md`](docs/DESIGN.md).

## [Unreleased]

## [0.7.0] - 2026-11-07

### Added

- Skip recaps, intros and previews by themselves: a playback preference (`auto_skip` on
  `/me/playback`, off by default) set in Your account → Playback. The web player then passes
  over each once a play; credits are never skipped this way.
- The badge a poster tile wears ("New" for 14 days, or "N unwatched" on a series being watched)
  now shows on the hero, on wide tiles (Continue watching, Next up) and on the cards of the All
  shows, All anime and All movies grids too.
- Cast and crew pictures on a title's Cast and crew page: a small round picture beside each
  name, loaded as it scrolls near, or the person's initials when there's none.
- Ask for it on a collection's page, under each film that isn't in the library and that nobody
  has asked for (while trending and similar titles are on, which lists those films). What
  asking said shows under the film, and its state turns to Requested.
- An album's page says how often you've played its songs and how often you finish what you
  start (from 3 finished-or-skipped listens, as the Aoide apps show it); an artist's page says
  the same for their songs and lists your top five of them.
- `GET /music/replay` takes `album_id` or `artist_id`, and says how many listens finished.
- From the start beside Resume on a title's page, for a movie or an episode partway through.
- The web player shows how far a jump went on the picture: "+10" or "−10" on the side it went,
  adding up over a run of jumps ("+30"), from the buttons and from J, L and the arrow keys.
  Screen readers hear the total once the run settles.
- Languages of their own for movies, shows or anime, per user: under Your account → Languages,
  each kind follows your languages unless you tick it and give it its own (anime with Japanese
  audio and English subtitles, say). Picking tracks uses the file's kind's own languages when it
  has them. `GET`, `PUT` and `DELETE /me/languages/{kind}` read, set and drop them, for the apps
  too.
- Watch's Home, its Shows, Anime and Movies pages (rows and the All titles grid) and My List
  follow the server's live updates: a title that comes in or goes, or a change to My List on
  another device, shows without a reload. They're drawn again in place, keeping the keyboard's
  focus and each row's scroll; the hero stays as it is.
- Send a playback report from the web player: Settings in the player, or Your account for the
  last thing played in that tab. It holds what the player saw (waiting and stalls, errors, how
  it played, conversion errors, seeks, subtitle changes) and goes to the admins' Diagnostics
  only when sent.
- Play the Watch home's previews with sound: a choice in Your account → Playback, kept by the
  browser. Previews start with sound once you've clicked or pressed a key on the page, as
  browsers allow, and muted before that.
- A quality limit in the web player's Settings panel: Original, or up to 40, 20, 10, 6 or
  3 Mbps, kept by the browser. A file over it is converted to stay within it; the player says
  so under the video.
- Replay under Listen: this month, this year or all time in figures (plays, time listened,
  songs, artists, albums, the busiest day), the top songs (which play), artists and albums, and
  every listen of the period, each of which can be deleted, or the whole period at once. Plays
  are counted as the Aoide apps count them, from every device's listens and the web's.
- `GET /music/replay` gives those figures and top lists for any window.
- Listen home's "Most played lately" says how often each song was played.
- Your requests come in stages (On the way, Waiting for approval, In the library, Declined),
  each with its poster or cover, who approved or declined it and when ("Approved by alice 2
  days ago"), and for one on its way where it's coming from and when that last changed ("From
  a torrent, updated 5 minutes ago").
- Ask for something's results show their posters and covers.
- Pick up a few seconds before where you stopped: Your account → Playback offers where you
  stopped, or 5, 10, 15 or 30 seconds earlier, kept by the browser, so the web player doesn't
  start mid-sentence.
- Watch's Home appears at once from a copy of its last answer kept in the browser, then
  refreshes; when the refresh fails the copy stays on screen. The copy is kept per user and
  forgotten on signing out, or whenever the browser finds its session ended.
- When a Soulseek download of a wanted album or song fails once slskd has it (the peer refused
  a file, it timed out, errored or was cancelled, or slskd lost it), the next best accepted
  Soulseek result of the same search is grabbed, from a user the album or song hasn't failed
  with, up to 3 downloads from a search. The failed download says the next source is being
  tried (2 of 3), and the new one says why while it downloads; when the tries are used up or
  no other user's copy is left, the failed one says so.
- A tile whose title the Watch hero shows (after a moment's hover or focus) plays its preview
  there too after a few seconds, by the hero's rules: a muted clip of the title, only on the
  home network, as the viewer's hero trailers setting allows, and never when the device asks
  for less motion.
- A subtitle size in the web player's Subtitles panel: Small, Medium, Large or Extra large,
  kept by the browser, for plain and styled subtitles alike.
- A song's actions on the web have Not interested (kept out of mixes, radio, shuffle and smart
  playlists; set on the song playing, it moves on) and Don't count plays, the same flags the
  Aoide apps keep, so a change on any of them shows on the others.
- Hidden songs under Listen lists the flagged songs, each flag clearable one by one or all at
  once.
- `GET /music/flags` and `PUT /music/tracks/{id}/flags` read and set them, written into the
  listener's sync log as a device of theirs would write them.
- A title's page plays a muted preview behind its header after a few seconds in view, as the
  Watch home's hero does and by the same rules (the viewer's hero trailers setting, clips only
  on the home network, nothing when the device asks for less motion, Unmute and Stop): the
  title's trailer when it's sharp enough (at least 1080 lines, or as tall as the title's own
  file), else a clip of the title. It stops when the header scrolls out of view.
- A search page for movies and shows (Search in the palette, and its last result, See all
  results): the library's matches with their posters, then, while trending and similar titles
  are on, the titles TMDB has that aren't in the library, each with Ask for it.
- `GET /api/v1/discover/status`: whether trending and similar titles are on, asking nothing of
  TMDB, so an app can leave out what they give while they're off.
- Trending this week, movies and shows from TMDB as posters, on Requests → Ask for something
  and on the search page before anything is typed: a title in the library opens its page, any
  other has Ask for it. Shown only while trending and similar titles are on.

### Changed

- The Watch hero's previews play only while the hero can be seen; scrolled away, a pick's
  trailer stops, and starts again when it's back in view.
- A listen from the web UI (or any app that sends listens through the music API) is also
  written into the listener's sync log as an Aoide play event, so it counts on their Aoide
  devices: Replay, finish rates, Top songs and play-count rules. Reading the log doesn't keep it
  a second time. Listens from before this release aren't copied.
- A song flagged "Don't count plays" (on an Aoide device) has no new listens kept from the web,
  as the apps already do; `POST /music/plays` answers 409.

### Fixed

- A playlist import finds a song MusicBrainz lists under a slightly different title, such as
  "The Rare Old Mountain Dew" for "Rare Old Mountain Dew": for a track with an artist, a
  recording by that artist whose title is the same but for a leading "the", "a" or "an" or a
  bracketed part, or shares most of its words, is taken when none has its own title, and its
  words are searched for once more when its title as a phrase finds nothing. A track without
  an artist still needs its exact title.
- A playlist item waiting on a song request fills once the request's download is imported,
  even when the file is tagged with another MusicBrainz recording of the song, or none: the one
  track the download put in the library counts as the song, as the request already showed it
  in the library.
- A subtitle track titled as anime's signs and songs track ("Signs & Songs", "S&S", "Signs"),
  or a subtitle file named `.signs.`, counts as forced, so with English audio and "when the audio
  isn't mine" it's the one picked, as a track flagged forced already was. Before, these tracks
  weren't flagged in most releases, so none was picked.
- The pickers in the web player's panels showed light text on white; they're on the stage's
  grey now. On a phone, a panel no longer runs past the top of the picture: it scrolls.

## [0.6.0] - 2026-11-06

### Added

- A torrent that Deluge says is downloading but that hasn't moved for 4 hours is replaced: its
  title (or album) is searched for again, passing over its release, and when another release
  is grabbed the stalled one is removed from Deluge with its data and its release blocklisted.
  With nothing else to grab it keeps going, says so, and is looked at again 4 hours later.
  Paused and queued torrents aren't stalled.
- On the web Watch home, each Continue watching and Next up tile that plays has a "Go to
  series" (or "Go to movie") link under it, to choose another episode or see the title. The
  row stays one Tab stop: Tab goes from a tile to its link, then on past the row.
- Before an artist's music is searched for, and after following finds new releases, each
  single or EP is compared with the artist's albums on MusicBrainz: one whose every song is on
  an album (the same recording, or the same title but for an edit or version note like "Radio
  Edit") is left out, so its songs don't come in twice. One with a song of its own (a B-side, a
  remix, a live take) is wanted whole. An album or single asked for by itself is never left out.

### Changed

- Asking for an artist (or following one) wants all their music: albums, EPs, singles, and live,
  compilation, remix and soundtrack release groups; only spoken word and broadcasts stay out.
  Before, only studio albums were wanted, so an artist with one album and many singles got one
  album.

### Fixed

- An album whose tracks mostly carry a MusicBrainz release id, with one that doesn't (a bonus
  track the release doesn't list), is one album again rather than two of the same name in the
  same folder. The extra entry goes at the next scan.
- A season pack (or a movie) with extras beside its videos imports: creditless openings and
  endings (NCOP, NCED), PVs, CMs, menus, trailers, and what's in an Extras, NC, Featurettes or
  Bonus folder go in the title's `Extras` folder, and the running-time check passes them over.
  Before, one extra without an episode number held the whole pack back and blocklisted it.

## [0.5.0] - 2026-11-05

### Added

- Melete's own profile pictures: sixteen, the Muses and their companions in neon line art, shown
  first in the gallery.
- Anime episodes without styled subtitles (no ASS/SSA track in the file and no ASS/SSA file
  beside it) are noted in Anime's health, one note per series (`anime_unstyled_subtitles`),
  and marked on the series' Manage page. `GET /api/v1/series/{id}/unstyled-subtitles` lists
  them with the release group of each copy, and a series search takes `avoid_group` to turn
  that group's releases down; Manage's "Find a release with subtitles" searches so and lets an
  admin choose.
- Artwork for large screens: the same file can be asked for at a larger size of its kind
  (posters at w780; backdrops, stills and logos at `original`; people's pictures at h632), so
  a 4K TV gets sharp art. A title's own images from its folder are made at those sizes too,
  up to 3840 wide.
- Continue Watching and Next Up items carry the title's `backdrop` and `backdrop_focus`, so a
  movie has a wide picture for a wide tile and an episode without a still falls back to its
  series'.
- A player can say which subtitle track the viewer chose (`subtitle`) and whether it draws
  ASS/SSA itself (`renders_ass`) when it asks for a conversion. When it can't and the track is
  styled (signs, positioning, karaoke), Melete burns the track into the picture, as
  `burn_subtitle` does, instead of leaving the player its WebVTT version; a request to copy the
  video keeps copying and says so. The answer gives `burned_subtitle` and `burn_reason`. A
  player that doesn't send `renders_ass` gets what it got before.
- **ASS/SSA subtitles as they are, with the video's fonts**, for players that draw ASS
  themselves (positions, signs, fonts, fades and karaoke as authored). The subtitle list marks
  each ASS/SSA track with `ass` and gives its `ass_url` (`subtitles/{track}.ass`, beside the
  stream URL as the WebVTT one is): an embedded track is copied out of the video unchanged once
  and cached, a file beside the video is sent byte for byte. `GET /api/v1/stream/{token}/fonts`
  lists the fonts the video carries (Matroska attachments: name, type, size and URL) and
  `GET /api/v1/stream/{token}/fonts/{font}` serves each, both under the stream link's token.
  The WebVTT version is unchanged.
- Movie releases named as converted to a high frame rate (`48`, `50`, `60` or `120 fps`,
  `interpolated`) are turned down, since their made-up frames can stutter on some players.
  Settings → Outside services → Deluge has a switch to take them anyway, and each movie's Manage
  page one to allow them for that film. Films really made at a high frame rate (the Hobbit
  films, Gemini Man, Billy Lynn's Long Halftime Walk) start allowed; series aren't affected.
- **A cover palette:** an album's `colours` now carry `palette`, the cover's main colours (up
  to five, the most of it first), for what's around a square cover on a screen of another
  shape, such as a full-screen player, rather than stretching it. Colours worked out before
  are worked out again once.
- Album covers at the size an app draws them: `/api/v1/music/albums/{id}/cover?size=160`
  (or 400, 600, 800), a JPEG made once from the cover and kept, never larger than the cover.
  A song row no longer downloads a large folder image.
- Each Watch home hero pick carries a `clip` beside its `trailer`: the file, where to start and
  for how long (40 seconds) to play a clip of the title itself, worked out as the web UI does
  (from where the viewer left it, else an eighth of the way in). An app chooses per screen
  which to play, and no longer works the clip out itself.
- A movie whose copy plays at 48 fps or more is noted on its page. When the film isn't one high
  frame rate releases are wanted for, the note says it's most likely made up from the film's 24
  and may stutter, with a link for admins to search for another release, and Manage's health
  lists it. Apps get it as `high_frame_rate` (`{fps, allowed}`) on `GET /api/v1/movies/{id}`.
- Each movie's Manage page has a switch to allow releases at a high frame rate for that film,
  for films really made so; `GET /api/v1/high-frame-rate-films` and
  `PUT /api/v1/high-frame-rate-films/{tmdb_id}` do the same for apps.
- **Compress** on the Make room page: a title's H.264 (or older) video files, Dolby Vision
  aside, converted to HEVC one at a time while nothing else is converted, on the GPU when it
  works, audio, subtitles and fonts copied and HDR kept. A copy takes its file's place (as
  `.mkv`, keeping watching progress) only if it plays as long with the same tracks and is at
  least a tenth smaller; the old file goes to the trash for 30 days. The page lists
  compressions under way and what they've freed.
- **Make room page** in Manage's Movies, Series and Anime: the service's suggestions, the
  strongest first, with what each would free and why, ticks to choose several and the space
  they'd free together. **Move to trash** removes a title: every file in its folder goes to
  the library's trash for 30 days, where Replaced files can put each back, and a wanted title
  stops being monitored so it isn't downloaded again. Swapping and compressing are listed but
  not available yet.
- **Make room suggestions** (`GET /api/v1/admin/make-room`, admins): movies and series to
  delete, swap for a smaller release, or compress, the strongest first, each with the space it
  would free and why, and the total. Nothing anyone is watching, has on their list or requested
  lately is suggested; a major studio's titles are suggested less (Settings → Make room adds
  studios to the built-in list). Suggestions only: acting on them comes later.
- Titles' companies from TMDB (studios and networks), kept with their details; titles fetched
  before are asked for theirs once.
- **Swap for a smaller release** on the Make room page, for titles on the wanted list: a
  movie, or each episode file of a series, is searched slowly (one search at a time, at least
  15 seconds apart, an indexer at its limit left out for an hour) for a release at most 1080p
  and clearly smaller (at most three quarters of the copy and 4.5 GB an hour). The best is
  grabbed, and the usual check and replace puts the old file in the trash once the new one
  plays as long. The title stops being monitored, so nothing upgrades it back. The page lists
  swaps under way, and says when downloads are off and swaps wait.
- Downloaded trailers play wherever a trailer in a title's folder does: in the Watch home's
  hero (`trailer.source` is `downloaded`) and first in the title's extras, as "Trailer", when
  its folder has none. Each plays through the file routes by its id, held to the same rules as
  an extra: seen by whoever may see its title, converted for a player that can't play it.
- **Trailer downloads** (off by default): each movie's, series' and anime's trailer, picked
  from TMDB's list of its videos (official, in the title's own language, then the metadata
  language, then English), downloaded once from YouTube by a new trailer downloader
  (`compose.trailers.yaml`, the `-trailers` image) that runs behind `compose.soulseek.yaml`'s
  VPN and won't start without it. Converted once on the GPU to HEVC (HDR kept) unless it's
  already H.264 or HEVC, at most the quality set in Settings (4K, 1080p or 720p), and kept in
  the config folder while the title is in the library; a title with a trailer in its folder
  gets none. Progress shows in Tasks. Playing them comes in a later change.
- The web player draws styled ASS/SSA subtitles as they were made, with their signs, placement,
  fonts and karaoke, over the video, instead of their WebVTT version. It uses JASSUB (libass in
  WebAssembly), served by Melete with the fonts the video carries; nothing is fetched from
  anywhere else. Where a browser can't draw them, the WebVTT version shows, as before.

### Changed

- An ASS/SSA file beside the video comes back from its `ass_url` in UTF-8, so players whose
  libass can't convert text encodings can read it. A file in another encoding (Windows-1252,
  Shift_JIS…) is converted once, read the way the WebVTT version reads it, and cached until the
  file changes; a UTF-8 file is still sent byte for byte, less a byte-order mark. Styles and
  override tags are untouched. A file whose encoding can't be told for sure is sent as before.
- A request's download progress moves every few seconds: Deluge is asked every 5 seconds while
  something is downloading (every minute otherwise), and at once when a download starts.
- A title's own poster and backdrop (`poster.*`, `fanart.*` in its folder) are cropped to the
  shape of their kind, 2:3 and 16:9, around the picture's subject, so an image of another shape
  never comes out stretched or letterboxed. They get new artwork URLs at the next scan, so apps
  fetch them again.
- The web player has its own controls over the picture, in full screen too: the seek bar with
  its previews and chapters, play, back and forward 10 seconds, the sound, the time, panels for
  Subtitles, Audio and Settings (speed, chapters), picture in picture and full screen. They fade
  while the video plays and come back on any movement, touch or key. The pickers and the Full
  screen button under the video are gone; a phone shows what fits.
- **Add new says which albums Soulseek's users share.** In Manage → Music → Add new, while
  slskd is set up, the one Soulseek search made when typing stops now serves albums as well as
  songs: each of MusicBrainz's albums that users share as a whole album folder says by how
  many and in which formats, and the Albums list is then ranked by how close each is to the
  words, as Songs is. Only albums MusicBrainz knows are listed, and adding one works as before.
  Only the words go to slskd. For the apps, `POST /api/v1/requests/soulseek/candidates` (admins)
  takes the words and MusicBrainz's albums for them, and answers Soulseek's songs and the albums
  its users share, from one search.
- In a browser that can't draw ASS/SSA itself, choosing an ASS/SSA track converts the video so
  the server can draw a styled one into the picture (`renders_ass: false`), instead of showing
  only its words. Browsers that draw ASS ask for nothing new, so changing subtitles doesn't
  restart their conversion.

### Removed

- **Motion covers made from still covers**, and their renderer (`compose.motion.yaml`, the
  `-motion` image): they rarely looked right and took space. The videos made are deleted from
  the config folder on the first start, and their records with them. Gone from the API:
  `/api/v1/music/motion-covers`, `/api/v1/music/motion-covers/status` and albums'
  `motion_cover_generation`; the Motion covers setting and task. An album's own motion cover,
  from its folder, is still given as `motion_cover`. To tidy up: take `compose.motion.yaml` out
  of `COMPOSE_FILE`, remove the `MELETE_MOTION_*` lines from `.env`, and delete the motion jobs
  and models folders.

### Fixed

- AI picks, search by description and music by description could come back empty with a model
  that thinks before answering: its thinking used up the room left for the answer. Melete now
  asks OpenRouter for no thinking (kept low for a model that can't turn it off), leaves more
  room, and an empty answer is logged with the model OpenRouter used and why it stopped.
- An indexer that answers a web page where its feed should be is now reported with what the page
  says ("the indexer answered a web page, not a feed: Feed disabled") instead of an XML parse
  error. AnimeTosho's feed answers this way since it was turned off.

## [0.4.0] - 2026-11-02

### Added

- Four more rows on the Watch home and in `GET /api/v1/home`: **New episodes** (shows whose
  next episode came in during the last two weeks, after you last watched them; they leave Next
  up while they're new), **Next in the collection** (the next film of a collection you've
  started), **Highly rated, not seen yet** (titles TMDB scores 7.5 or more, in genres you watch)
  and **Watch again** (what you finished more than six months ago). TMDB's score is now kept
  with each title's details; titles fetched before get theirs once, in the background.
- The Movies, Shows and Anime pages have their own "Because you watched" rows, from the titles of
  that kind you finished, and keep "Featured for you" and your rows to that kind. Apps get the
  same with `GET /api/v1/home?kind=movies`, `shows` or `anime`.
- **Profile pictures.** Everyone can upload their own on Your account, or choose one from a
  gallery: Melete's own pictures (`avatars/`), then a folder on the server the household fills
  (`compose.avatars.yaml`, `MELETE_AVATARS_DIR`), which jf-avatars' community collections fit as
  they are. It shows beside your name in the header, and the apps get it as `avatar_url` on
  `/me`, the users list, the user directory, each person in the household's listening and each
  member of a listen-together session.
- **Music by description, with the server's OpenRouter key.** `POST /api/v1/ai/music/search`
  turns a phrase such as "upbeat rock from the nineties" into filters (genres the library has,
  a year range, words to search for, liked only, an order), and `POST /api/v1/ai/music/mix`
  turns a mood into smart-playlist rules in Aoide's own format, with the names it mentioned and
  each rule left out with its reason. The model never picks tracks: the apps run the filters and
  rules against the library themselves, so they no longer need an OpenRouter key of their own.
  What goes to OpenRouter: the words typed and the names of up to 300 of the genres the asker
  may see. Each call counts as one of the user's 30 AI searches an hour.
- Manage → System → Tasks shows making motion covers and splitting tracks into parts for the
  Auto DJ: how far each has got ("making motion covers: 120 of 840") while it works through the
  library, and how the run went once it has caught up.
- `compose.libretranslate.yaml` and `compose.flaresolverr.yaml`: a LibreTranslate for lyrics and
  a FlareSolverr for trackers behind a browser check, each run beside Melete by adding its file
  to `COMPOSE_FILE`, with Melete pointed at it. Neither publishes a port.
- Aoide playlists can hold songs that aren't in the library yet. Such an item is pending: it
  names the song and the request that will bring it, and once the song arrives Melete swaps the
  track in, in the sync log, so every device just syncs it. A playlist import can make the
  playlist too (`make_playlist`): every track goes in, in order, those the library has as they
  are (and they aren't asked for), the rest pending on their requests, or on none with why when
  MusicBrainz had nothing that fits. The import says which playlist (`playlist_id`) and how many
  tracks were in the library already (`in_library`). `GET /api/v1/playlists/{id}` and the web's
  playlist page show a pending song with how its request stands.

### Changed

- **Add new finds songs on Soulseek beside MusicBrainz.** In Manage → Music → Add new, while
  slskd is set up, Soulseek is searched for the same words once typing has stopped (for 1.2
  seconds, and 3 letters or more), not only when MusicBrainz finds nothing. MusicBrainz's
  results show as soon as they come; Soulseek's songs join the Songs list when they arrive and
  the list is ranked by how close each song is to the words, with a MusicBrainz or Soulseek
  badge on each. Only the words go to slskd, as before. For the apps, every search result now
  has a `closeness` (0 to 100), and `GET /api/v1/requests/soulseek/candidates` (admins) gives
  Soulseek's songs as search results with theirs, so the two can be mixed.
- "Because you watched" shows up to three rows even when a title you finished recently has
  nothing like it in the library: it looks further back instead.
- The Watch home's previews play at full quality. A trailer in the title's folder is used only
  when it's at least 1080p (or as sharp as the title itself); otherwise the hero plays a clip of
  the title. The web plays either as the player would (the file as it is, its video copied, or
  converted at its own height) rather than converted to 720p.
- Motion covers are made up to 1440 pixels (was 1080), sharp across a phone when an app shows
  them full width at the top of its player. Existing ones are made again in the background, one
  album at a time.

- **Starting Melete doesn't scan the libraries.** A new install waits for Scan all now (Settings,
  Libraries), so you can check the folders first; after that, scans follow the schedule counted
  from the last one, so a restart scans only when a scheduled scan came due while Melete was
  down.

### Removed

- **Trailers online.** The setting, the trailers fetched from TMDB's video lists and played
  through YouTube's player, and `/api/v1/movies/{id}/trailer` and `/series/{id}/trailer` are
  gone, and the video lists kept for them are deleted. Trailers in a title's folder still play
  on its page and in the hero, and the hero's `trailer` is only ever `folder` now.

### Fixed

- `GET /api/v1/ai` (whether AI is on) answers through every way in, so a music app on a way in
  without video can offer music by description and mood mixes.
- Setting a playlist's picture from a sandboxed browser said "Melete didn't answer": the browser
  lost sight of the file while sending it. The page now reads the picture as soon as it's
  chosen and says plainly when it can't, and sends it as the type its bytes are, so a PNG named
  ".jpg" is taken.
- HEVC from an MP4 that keeps its parameter sets only in the stream (often tagged `hev1` or
  `dvhe`) is no longer copied: the copy's sample entry listed none, which Apple's players may
  refuse. The probe now notes such a stream (`parameter_sets_in_stream_only` on the file's
  video), copy mode refuses it with "ask for hevc or h264", and the web player converts it
  instead. HEVC MP4s probed before are probed again. Matroska files copy as before.

## [0.3.2] - 2026-11-02

### Changed

- **The motion cover renderer and the stem separator are published with each release**, as
  `ghcr.io/virel0/melete:<version>-motion` and `:<version>-stems`, and `compose.motion.yaml` and
  `compose.stems.yaml` run those. Turning motion covers or the Auto DJ on is now adding the file
  and its folders in `.env` and restarting, with no image to build. `MELETE_MOTION_IMAGE` or
  `MELETE_STEMS_IMAGE` names another, such as one built from `motion/` or `stems/`.

### Fixed

- **Soulseek found nothing, for anything.** Every search gave up 10 milliseconds after it started:
  slskd takes its search timeout in milliseconds, though its API docs say seconds, and Melete
  sent 10. Searches now wait 10 seconds after the last answer, as meant.
- Indexers are asked for a compilation by its title alone (without "Various Artists" and the
  bracketed part), so a soundtrack's torrents can be found.
- A feed with no API key that turns Melete away (HTTP 401 or 403), as AnimeTosho's does, is asked
  again through FlareSolverr when one is set up. Feeds with a key never go through it.

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
[Unreleased]: https://github.com/Virel0/Melete/compare/v0.7.0...HEAD
[0.7.0]: https://github.com/Virel0/Melete/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/Virel0/Melete/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/Virel0/Melete/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/Virel0/Melete/compare/v0.3.2...v0.4.0
[0.3.2]: https://github.com/Virel0/Melete/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/Virel0/Melete/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/Virel0/Melete/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/Virel0/Melete/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/Virel0/Melete/releases/tag/v0.1.0
