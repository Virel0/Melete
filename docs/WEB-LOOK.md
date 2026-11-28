# How the web UI looks

This is the look the web UI is moving to, one pull request at a time, as planned in
[the redesign issue](https://github.com/Virel0/Melete/issues/472). It covers every page:
the rules first, then what each page does with them. A pull request that brings a page closer
to it doesn't need to explain why. A pull request that changes a rule updates this file too.

Values are given in pixels where they come from the mockups. Once a value is a token in
`src/web/app.css`, the token is what counts, and pages use the token, never the number.
Where the mockups use a few values close together (corners of 10, 12 and 14 px), this file
says which one to keep.

Today's pages are not the guide for how things look. They are still the guide for how things
work: labels, keyboard use, focus, every state, and pages built from DOM nodes.

## The rules in short

1. **One primary action per screen.** It is the only filled accent button. Everything else is
   secondary (a tonal fill), tertiary (text and icon) or an icon button.
2. **Play is always the accent.** The title's colour stays in the page's fade, never on a
   button.
3. **Tone instead of lines.** Surfaces are told apart by their fill. Borders stay on inputs
   and where a line means something: the active tab, a divider between tabs and content.
4. **One shell for the three sides.** The same header everywhere; a sidebar only for what is
   under a section; a bar at the bottom on phones.
5. **Art carries the tile.** One line of text under it, progress drawn on it, actions shown
   on hover, focus and long press. Missing art gets a drawn placeholder.
6. **Pills for anything you press, rounded rectangles for anything you look at.**
7. **A finding about many titles is one line** with a count and one action.

## Colour

The dark theme is the target drawn so far. Each new token needs a light value too, chosen in
the pull request that adds it, with its contrast pair commented for `check_ui.py`.

| Role | Token | Dark | Used for |
|------|-------|------|----------|
| Page | `--bg` | `#121417` | The page, the header, ink on accent fills |
| Sunk surface | new: `--surface-sunk` | `#16181c` | Sidebars, rows in a long list, the loading, empty and failed panels |
| Surface | `--surface` | `#1b1e23` | Cards, the side switch's track, the now-playing bar, the phone bar |
| Raised | `--surface-raised` | `#252930` | Secondary buttons, icon buttons, selects, the current sidebar entry |
| Raised, hover | new: `--surface-hover` | `#30353e` | A secondary or icon button under the pointer |
| Line | `--line` | `#343944` | The line under in-page tabs, the empty part of the now-playing bar's position |
| Strong line | `--line-strong` | `#6b7080` | Input outlines |
| Text | `--text` | `#ece9e2` | Text, the fill of a pressed toggle and a chosen chip |
| Soft text | new: `--text-soft` | `#d6d3cc` | Running text under a heading: overviews, descriptions, a finding's detail |
| Muted | `--muted` | `#a4a29b` | Metadata, labels over groups, entries not chosen, time left |
| Accent | `--accent` | `#e3a857` | The primary button, progress, the current entry in the phone bar |
| Accent, hover | new: `--accent-hover` | `#edbb74` | The primary button under the pointer |
| Danger | `--danger` | `#f09484` | A failure's own action, Sign out, problem counts |
| Danger tint | new: `--danger-tint` | `#2a1f1e` | The card behind a failure |
| Accent tint | new: `--accent-tint` | `#2a2418` | A notice about something you are part of: the party you are in |
| Skeleton | new: `--skeleton` | `#1f2228` | Shapes standing in for content that is loading |
| On art | new: `--on-art-fill` | `rgb(236 233 226 / 0.14)` | Secondary buttons over artwork |
| On art, track | new: `--on-art-track` | `rgb(236 233 226 / 0.25)` | The empty part of a progress bar on a tile |

State colours (`--state-have`, `--state-coming` and the rest) and the stage tokens stay as
they are, and so does the rule that a state always has its shape as well as its colour.
`--stage-muted` (`#b4b1a9`) is the metadata colour on a hero, over art.

Contrast, measured on the dark values: `--text-soft` on `--bg` is 12.3:1 and on `--surface`
11.2:1; `--muted` on `--surface-raised` is 5.7:1; `--bg` on `--accent-hover` is 10.5:1 and
on `--danger` 8.2:1.

## Type

Headings use the body font, set apart by size and weight. Whether headings get a font file of
their own is still open.

| Step | Size | Weight | Used for |
|------|------|--------|----------|
| Hero | 52 px (38 px on phones) | 750, line height 1.02, letter spacing -0.01em | A title's name over its art, the featured title on a home |
| Page | 40 px | 750, line height 1.1 | A page's heading: Movies, Your account, Good evening |
| Section | 20 px | 700 | A row or section on a page: Continue watching, Needs attention |
| Card | 18 px | 700 | A heading inside a card or panel |
| Item | 17 px | 650 | An episode's title, a hero's overview (400) |
| Body | 16 px | 400 | Running text, buttons, header entries |
| Small | 14 px | 600 for a name, 400 for the rest | Text under a tile, list rows, secondary buttons in dense rows |
| Smallest | 13 px | 600 for labels | Metadata, group labels in a sidebar, the side switch |

The type pull request replaces `--step-1` to `--step-4` with these steps. Weights: 750 for
hero and page headings, 700 for section headings and primary buttons, 650 or 600 for names
and secondary buttons, 400 for everything else.

The wordmark is a drawn mark (two arches in the accent) next to "Melete" at 19 px, weight
700.

## Space and shape

Spacing stays on the `--space-*` scale (4, 8, 12, 16, 24, 32, 48, 64). The mockups also use
6, 10, 14 and 18 px between small items; those round to the nearest step.

- Page edges: 24 px on the sides of the header and of pages without a sidebar; 32 px inside
  a page next to a sidebar.
- Between sections on a page: 32 to 36 px. Between a section heading and its row: 12 px.
- Between tiles in a row: 16 px; in a grid: 20 px.

| Corner | Value | For |
|--------|-------|-----|
| `--radius-s` | 4 px | Very small art: a 32 by 48 poster in a list row |
| `--radius-m` | 8 px | Episode stills, sidebar entries, small thumbnails |
| new: `--radius-l` | 12 px | Tiles, posters, covers, cards, list rows, inputs |
| new: `--radius-xl` | 18 px | Panels and dialogs: sign-in, a guest's invite, the empty and failed states |
| new: `--radius-pill` | 999 px | Every button, chip, toggle, switch and avatar |

Shadows only where something floats over the page: a dialog (`0 24px 64px` at 50% black)
and the seek preview in the player. Cards don't have them.

## Buttons

| Level | Look | Height | When |
|-------|------|--------|------|
| Primary | `--accent` fill, `--bg` text, weight 700 | 48 px on a hero, 44 px elsewhere | The one thing the screen is for: Play, Continue here, Join the party, Sign in |
| Secondary | `--surface-raised` fill, no border, weight 600 | 44 px, 40 px in dense rows | Other actions worth a button: From the start, Search for other releases |
| Tertiary | No fill, text with an icon in front, weight 400 | 44 px | Actions that are there but not pushed: My List, Choose from the gallery, Take back |
| Icon | Round, `--surface-raised` fill | 44 px square | Search, Mark watched, a tile's menu |
| Danger | `--danger` fill for a failure's own action; `--danger` text with no fill for Sign out | as above | Only when the action deals with a failure, or ends something |

- Over artwork, secondary and icon buttons use `--on-art-fill` instead of `--surface-raised`.
- Hover: primary goes to `--accent-hover`; secondary and icon to `--surface-hover`;
  tertiary gets a `--surface` fill.
- Unavailable: the fill drops a step (primary to a dim accent, `#4a4236` in the mockups; secondary to `--surface`) and
  the text goes to `--muted` or below. Prefer leaving a button out over showing it
  unavailable.
- A toggle that is on (Watched, a chosen filter) is filled with `--text` and its ink is
  `--bg`, with `aria-pressed`.
- An icon button keeps its name: as `aria-label`, and as a tooltip on hover and focus.
- Horizontal padding: 24 px on 48 px buttons, 20 px on 44 px, 16 px on 40 px, 12 px on
  tertiary.

A group of tertiary buttons can follow the primary and secondary ones after a thin
divider (1 by 28 px, `--text` at 20%), as on a title's page.

## Focus

The focus ring stays as `app.css` has it: 3 px of `--accent`, 2 px out. On a fill that is
already the accent (the primary button), the ring is `--text` instead. A
focused tile gets the ring round its art.

## The shell

### The header

One row, on every side, in this order:

1. The wordmark.
2. The side switch: Watch, Listen and Manage as a segmented pill on a `--surface` track.
   The current side is filled with `--text` (ink `--bg`, weight 650), the others are
   `--muted`. 13 px, 6 by 14 px padding.
3. The side's sections as text links, 8 by 12 px padding. The current one is `--text`
   at weight 600, the others `--muted`. Manage's services carry a count pill (20 px high,
   12 px, weight 700) in `--danger` for problems or `--accent` for waiting, with the count
   and what it counts as its label.
4. Pushed to the end: Search and the account, both 44 px round. The account shows the
   user's picture or initial.

The header has the page's colour and no line under it. When the sections don't fit, they move
to a row of their own under the header, never half onto a second row.

### The sidebar

Only where a section has pages of its own:

- Listen: Yours (Liked songs, Liked albums and the rest), Pinned and Playlists. Never the
  header's sections again.
- Manage: the current service's pages (Library, Add new, Needs a match, Make room, Calendar) and its
  Activity.

220 to 240 px wide, on `--surface-sunk`, 16 by 12 px padding. Entries are 15 px with 8 by
12 px padding and `--radius-m`; the current one sits on `--surface-raised` at weight 600.
Group labels are 13 px, weight 600, `--muted`. Pinned items show a 32 px picture before the
name.

Watch has no sidebar for now; whether it gets one on wide screens is open.

### On phones

- The header drops the wordmark and the sections, and keeps the side switch, Search and the
  account. Over a hero it floats on the art, with the side switch and Search on the page
  colour at 72%.
- The side's main sections move to a bar fixed to the bottom: four entries with a 24 px icon
  over a 12 px label, on `--surface`, with room under them for the home indicator. The
  current one is `--accent` at weight 650. On Watch they are Home, Movies, Shows and My List.
- Rows of tiles scroll sideways.

### The now-playing bar

On Listen, at the bottom of the page on `--surface`: the cover (48 px, 6 px corners) with the
song and artist on the left, the controls and the position in the middle. The position is a
4 px track in `--line` filled with `--text`, with the
times in 12 px `--muted` either side.

## Tiles

| Tile | Shape | Width in a row | Text under it |
|------|-------|----------------|---------------|
| Poster | 2:3 | 152 px | The title, 14 px, one line |
| Landscape (continue watching) | 16:9 | 272 px | The title (weight 600) and the time left (`--muted`), one line |
| Cover (albums, artists' releases) | 1:1 | 150 px minimum in a grid | The name (weight 650) and the artist (`--muted`), two short lines |
| Recent (Listen's Jump back in) | A row: 56 px cover then name | 240 px minimum in a grid | The name and what it is |

- Corners `--radius-l`, art fills the tile, no border and no shadow.
- Progress is a 4 px bar along the bottom of the art: `--accent` on `--on-art-track`.
- An episode code (S2 E4) sits on the art at the bottom left, 12 px weight 650, on the page
  colour at 78% with 6 px corners. Nothing else goes on the art except a pressed state.
- Actions (the ⋯ menu, Play on a cover, Like) appear on hover, on keyboard focus and on long
  press, as icon buttons on the art.
  They are never shown on every tile at once. On a cover, Play is a 44 px accent button at
  the bottom right and the menu a 36 px one at the top right, 10 px in from the edges.
- Missing art: the tile is filled with a dark colour taken from the title's name, with the
  title's initials (32 to 34 px, weight 750) in a lighter colour of the same hue at the
  bottom left. Posters add the full title under the initials. Never a blank grey square.
- A series shows once across Continue watching and Next up.

## Lists and cards

- Long lists (Manage's library, Songs, requests) are rows on `--surface-sunk` or `--surface`
  with 2 to 10 px between them and `--radius-l`, not a table with lines. Each row: a small
  picture, the name (weight 600) with its year or detail in `--muted`, then what the list is
  about (quality, state, length) on the right, then at most one action.
- Episode rows: a 200 px still (16:9, `--radius-m`, progress on it), the number in `--muted`
  before the title at 17 px, the length and date in 13 px `--muted`, two lines of overview
  in `--text-soft`, and Mark watched as an icon toggle at the end. The episode in progress
  sits on `--surface`; the rest have no fill.
- Cards (account sections, a quality profile, a service's settings) are `--surface` with 22 px
  padding and `--radius-l`; a card's heading is the card step.

## Title pages

Movies and series open the same way. People, collections, albums and artists have their own
shapes, in the table at the end.

- A hero at least 480 px high with the art behind it, under two scrims: from the left
  (page colour at 100%, 72% at 38%, clear by 72%) and from the bottom (page colour, clear by
  55%). The title's colour tints the page's fade as it does today.
- In the hero: the name at the hero step, metadata in 14 px `--stage-muted` separated by space
  (no dots or bars), the overview in 17 px, then the buttons: Play (or Resume, with the time
  left inside the button at weight 500), one or two secondary actions, the divider, then the
  tertiary ones.
- Under the hero, in-page tabs for what the old page stacked as buttons (Episodes, More like
  this, Extras, Cast and crew, About): text links with 12 by 6 px padding, the current one at
  weight 650 with a 3 px `--accent` line under it, all over a 1 px `--line`.
- A season picker is a select on `--surface-raised` with the season's count of episodes and
  watched ones next to it in `--muted`.

## Manage

- Each service's page starts with Needs attention, then its library.
- A finding is a card with the count and what it is in bold, why in `--text-soft`, and one
  button. A failure sits on `--danger-tint` and its button is the danger fill; anything else
  sits on `--surface` with a secondary button.
- A finding about many titles lists them in a `<details>` under its text, at most six names
  and "and N more", each name a link.
- Filters over a list are chips: pills 36 to 40 px high, `--surface` when off, `--text` fill
  with `--bg` ink when on, `aria-pressed`.
- States keep their shapes: a filled circle for in the library, a ring for on its way, a
  diamond for waiting, a square or triangle for missing or failed, as `app.css` defines.

## Forms

- A label above its field, 14 px weight 600.
- Inputs 44 px (48 px on sign-in and invites), `--bg` fill on a card or `--surface` on the
  page, 1 px `--line-strong` outline drawn inside, `--radius-l`, 16 px text.
- A search field is 56 px with a 2 px `--accent` outline while focused and a search icon
  before the text.
- Selects look like secondary buttons with a chevron, pill-shaped. (The season picker's
  10 px corners in the mockups snap to this.)
- A choice between three or four values (the theme) is a segmented pill like the side
  switch.
- A file upload is a secondary button with an upload icon; the browser's own file picker is
  never shown.
- Saving is a secondary button under the fields, or the primary one when the form is the
  screen.

## Loading, empty and failed

All three take the space of what they stand for, on `--surface-sunk` with `--radius-xl`.

- **Loading:** skeleton shapes in `--skeleton` where the content will be, the same size, so
  nothing moves when it arrives. A spinner only after a second.
- **Empty:** a 72 px round icon on `--surface-raised` in the accent, a heading at 22 px, one
  sentence in `--text-soft` saying how to fill it, and a secondary button to the place that
  does.
- **Failed:** the same shape with the icon on `--danger-tint` in `--danger`, a heading saying
  what didn't load, a sentence on why if known, and Try again as the primary button. It is a
  `role="alert"`.

## The player

The player is dark whatever the theme. Its states, its data and its decisions are planned on
the player's own issue; this is only its look.

- The picture fills the screen. Controls fade in over two scrims: black at 70% from the top
  (160 px) and at 85% from the bottom (260 px).
- Top left: Back to the title, a 48 px round button on the page colour at 55%. Next to it the
  series at 20 px weight 700 and the episode at 15 px in `--text-soft`.
- The seek bar runs the full width above the controls: a 4 px track that grows on hover, the
  played part in `--accent`, a 16 px `--accent` thumb, and a preview frame (192 by 108,
  `--radius-m`) with the time above the pointer. It is a `role="slider"` with the time as its
  value text.
- Controls are icon buttons with no fill, 48 px (Pause 52 px): Pause, back and forward 10
  seconds, Mute with an 88 px volume bar on the left; Next episode, Subtitles and audio,
  Settings, Watch together and Full screen on the right. Each keeps its name as a tooltip.
- Skip intro is the one filled button, above the controls on the right, in `--text` with
  `--bg` ink.
- Subtitles and audio opens as a dialog panel down the right side, 400 px wide and 16 px
  from the edges, on `--surface` at 97%, with `--radius-xl`. Choices are radio rows with a
  note under each name.
- On phones, the picture sits at the top at full width. Over it: back and forward 10
  seconds either side of a 64 px Pause on black at 50%, Full screen, the time and the
  progress. Under it: the series and episode, Subtitles, Together and Settings as three
  buttons on `--surface`, then Up next.

## Page by page

Each page follows the rules above. This table says which shape each page takes.

| Page | Shape |
|------|-------|
| Watch home | A featured hero (540 px) with Play, More info and an icon button for My List, and step dots at the bottom right; then rows: Continue watching as landscape tiles, then poster rows |
| Movies, Shows, Anime, My List | The page heading with a find field and a sort select (both pills) on its right; filter chips and a Watched toggle under it; a grid of posters |
| A series, a movie | Title page with tabs; episodes under the Episodes tab |
| A person, a collection | A round 168 px picture next to the name at 44 px, metadata and a short biography; then In the library as a poster grid, and Not in the library with Ask for it on each |
| Watch together | Page heading; a notice on an accent tint when you are already in a party, with Go back to it as the primary button; Join a party (a code typed into separate boxes) and Start one as two cards side by side |
| Search | A panel over the page, opened from the header or Ctrl K: a 20 px field with Esc shown on the right, results as options grouped under 13 px labels, each with a small picture |
| Listen home | Sidebar; Good evening at the page step; a card for music playing on another device with Continue here; Jump back in as recent rows; Recently added as covers; the now-playing bar |
| Albums, Liked albums | Sidebar, page heading with a sort select, a grid of covers |
| An album, a playlist | The page fades down from the cover's colour; the kind (Album) in 14 px, the name at the hero step, the artist as a link with year, songs, length and format; Play as primary, Shuffle as secondary, Like and a menu as icon buttons; songs as rows, the playing one marked |
| An artist, a composer | The name and Play, Shuffle and Radio; Most played as song rows; Albums and Singles as covers |
| Songs, Liked songs, Hidden songs | Page heading with Shuffle all as the primary button; songs as rows |
| Genres, Composers, Works, Artists | Genres as tiles at least 220 px wide with the name at 22 px weight 750, a count and two covers; artists and composers as round pictures |
| Replay | A segmented control for This month, This year and All of it; one 24 px sentence that sums up the period with its numbers in the accent; Top songs and Top artists side by side; Every listen as rows, each with a delete icon button, and a way to delete the period's listens |
| Now playing | A full-screen view: the song, artist and album, the position and the controls on the left; Lyrics and Queue as tabs on the right, with Translate and Timing for lyrics |
| Mixes, Radio | Mixes as cards with Play and Shuffle, and Make the mix as a secondary button; Radio as rows to start a station from |
| Manage: a service's library | Sidebar with the service's pages; Needs attention cards; the library as rows with chips |
| Manage: one title | A back link to the service; a 132 px poster beside the name at 36 px, its state in words and colour, and its details as a list of terms; Search for a release as primary, Search and choose as secondary; files, history and the rest as in-page tabs |
| Add new | A search, then the chosen title with its options as selects; Add (the title's name) as primary and Cancel as tertiary |
| Needs a match | Folder 1 of 3; the folder's path in a monospace font; the likely matches as cards on the sunk surface, each with This one; Skip this folder as secondary |
| Queue, History, Replaced files | A segmented control for the three; each download a card with its title, release and the steps it has been through, and a menu |
| Requests to decide | A count of waiting requests; one card per request with its art, who asked and when, the title and their note; Approve as primary, Decline as secondary |
| A quality profile | Qualities and Custom formats as cards; each format with its score, and a minimum score; Save profile as primary and Cancel as tertiary |
| Indexers | Test all as secondary and Add an indexer as primary; each indexer a row with its state, and a fix action on the ones that fail |
| Calendar | The month as the heading with previous and next icon buttons, Today, and Month or List as a segmented control; a seven-column grid with each release as a small entry |
| System status, Tasks | Status, Health, Room left and Background work as sections; tasks with Run now |
| Users | Add a user as primary; each user a row with a role select, an on and off switch, New password and Delete |
| Outside services and other settings | A sidebar of groups; the heading and one sentence on what leaves the server; each service a card saying what it sends, with Test and Save |
| Sign in | The art on one side and the form on the other, at most 360 px wide: the wordmark, Username and Password, Sign in as primary, then "or" and Sign in with a code from another device |
| Your account | Cards: profile picture, appearance, password, two-step sign-in, signed-in devices |
| Ask for something | Page heading with a link to your requests; the kind as chips; a 56 px search; results as posters with Ask for it, In the library or Asked for |
| Your requests | Rows: poster, title and kind, the state as shape and words, the admin's note if any, then Watch or Take back |
| A guest joining a party | A 440 px panel over the title's art, dimmed: who invited them and to what, a name field and Join the party |

## Still open

- The light theme's values for the new tokens.
- One font file for headings, served by Melete itself, or the system font throughout.
- A sidebar for Watch on wide screens.
