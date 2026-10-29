// Songs and Genres: every song, searchable and sortable, each playing the list from there; every
// genre with how much of it is here, and a genre's page, which plays its songs and shows its
// albums.

import { SignedOut, api, h, number, plural, show } from "/web/lib.js";
import { genreFilter, pagedList } from "/web/music-list.js";
import { albumCard, countedItem, markPlaying, songRow } from "/web/music-parts.js";
import { trackActions } from "/web/music-actions.js";
import { looseQueue, play, shufflePlay } from "/web/music-play.js";
import { emptyState, hold, placeholders, release } from "/web/page-parts.js";

const SONG_SORTS = [
  ["title", "Title"],
  ["recent", "Recently added"],
  ["album", "Artist and album"],
];
// A genre's page shows this many albums, and plays up to this many songs (the API's page).
const GENRE_ALBUMS = 60;
const GENRE_SONGS = 500;

export const genreHref = (name) => `#/music/genres/${encodeURIComponent(name)}`;

export async function songList(params, ctx) {
  const filters = await genreFilter(ctx, "tracks");
  if (!ctx.current()) return;
  // The songs shown so far, so a song plays the list as it stands from there.
  const known = new Map();
  const list = h("ul", { class: "tracks songs" });
  const playFrom = (id) => {
    const rows = [...list.querySelectorAll("[data-track]")];
    const tracks = rows.map((row) => known.get(row.dataset.track)).filter(Boolean);
    play(looseQueue(tracks), Math.max(0, tracks.findIndex((t) => t.id === id)));
  };
  await pagedList(
    { ...ctx, params },
    {
      title: "Songs",
      base: "#/music/songs",
      noun: ["song", "songs"],
      sorts: SONG_SORTS,
      defaultSort: "title",
      path: "/music/tracks",
      item: (track) => {
        known.set(String(track.id), track);
        return songRow(track, () => playFrom(track.id), trackActions(track, looseQueue([track])[0]));
      },
      list,
      filters,
    },
  );
  if (ctx.current()) markPlaying(list, ctx);
}

export async function genreList(ctx) {
  const title = h("h1", {}, "Genres");
  show("Genres", title, placeholders("genres"));
  const genres = await api("/music/genres");
  if (!ctx.current()) return;
  if (!genres.length) {
    show("Genres", title, emptyState("No genres yet. They come from the songs' tags once a music library is scanned."));
    return;
  }
  show(
    "Genres",
    title,
    h("p", { class: "status" }, plural(genres.length, "genre", "genres")),
    h(
      "ul",
      { class: "artist-list" },
      genres.map((g) => countedItem(genreHref(g.name), g.name, `${plural(g.albums, "album", "albums")}, ${plural(g.tracks, "song", "songs")}`)),
    ),
  );
}

// Play and Shuffle for a genre's songs, asked for when pressed.
function genreActions(name, total) {
  const said = h("span", { class: "note", role: "status" });
  const start = (how) => async (event) => {
    const button = event.currentTarget;
    if (!hold(button)) return;
    said.textContent = "";
    try {
      const page = await api(`/music/tracks?${new URLSearchParams({ genre: name, limit: String(GENRE_SONGS) })}`);
      how(looseQueue(page.items));
      if (total > GENRE_SONGS) said.textContent = `Playing the first ${number.format(GENRE_SONGS)}, by artist and album.`;
    } catch (err) {
      if (err instanceof SignedOut) throw err;
      said.textContent = `Couldn't play them: ${err.message}`;
    } finally {
      release(button);
    }
  };
  const hidden = h("span", { class: "visually-hidden" }, ` ${name}`);
  return h(
    "div",
    { class: "play-actions" },
    h("button", { type: "button", onclick: start((q) => play(q)) }, "Play", hidden.cloneNode(true)),
    h("button", { type: "button", class: "quiet", onclick: start(shufflePlay) }, "Shuffle", hidden),
    said,
  );
}

export async function genrePage(name, ctx) {
  const back = h("p", {}, h("a", { href: "#/music/genres" }, "All genres"));
  const title = h("h1", {}, name);
  show(name, back, title, placeholders(`${name} albums`, { kind: "cards" }));
  const [genres, albums] = await Promise.all([
    api("/music/genres"),
    api(`/music/albums?${new URLSearchParams({ genre: name, limit: String(GENRE_ALBUMS) })}`),
  ]);
  if (!ctx.current()) return;
  const genre = genres.find((g) => g.name.toLowerCase() === name.toLowerCase());
  if (!genre || !albums.total) {
    show(name, back, title, emptyState(`Nothing in the library is tagged ${name} now.`, { href: "#/music/genres", label: "See every genre" }));
    return;
  }
  show(
    name,
    back,
    title,
    h("p", { class: "status" }, `${plural(genre.albums, "album", "albums")}, ${plural(genre.tracks, "song", "songs")}`),
    genreActions(genre.name, genre.tracks),
    h("ul", { class: "grid" }, albums.items.map((a) => albumCard(a))),
    albums.total > albums.items.length
      ? h("p", { class: "more" }, h("a", { href: `#/music/albums?${new URLSearchParams({ genre: genre.name })}` }, `All ${number.format(albums.total)} ${genre.name} albums`))
      : null,
  );
}
