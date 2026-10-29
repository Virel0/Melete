// Listen's pages, by address: Home (where `#/music` opens), Albums, Artists, Songs, Genres,
// Composers, Works and Radio, Liked songs and Playlists, and the pages of an album, an artist, a
// genre, a composer, a work and a playlist. Built like the rest of the page: DOM nodes and textContent only, lists paged with
// "Show more" (music-list.js), what's shown kept in the URL.

import { h, plural } from "/web/lib.js";
import { albumPage, artistPage } from "/web/music-album.js";
import { genreList, genrePage, songList } from "/web/music-browse.js";
import { homePage } from "/web/music-home.js";
import { likedPage } from "/web/music-liked.js";
import { playlistPage } from "/web/music-playlist.js";
import { playlistsPage } from "/web/music-playlists.js";
import { genreFilter, pagedList } from "/web/music-list.js";
import { albumCard, artistCounts, countedItem, workItem } from "/web/music-parts.js";
import { radioPage } from "/web/music-radio.js";
import { composerPage, workPage } from "/web/music-works.js";

export { albumCard } from "/web/music-parts.js";

const ALBUM_SORTS = [
  ["artist", "Artist"],
  ["title", "Title"],
  ["year", "Year, newest first"],
  ["recent", "Recently added"],
];
const ARTIST_SORTS = [
  ["name", "Name"],
  ["recent", "Newest album"],
];
const COMPOSER_SORTS = [["name", "Name"]];
const WORK_SORTS = [
  ["composer", "Composer"],
  ["title", "Title"],
];

export function musicPage(parts, params, ctx) {
  const [, section, id] = parts;
  const one = /^\d+$/.test(id ?? "");
  // The album list was `#/music` before Home opened there: its old addresses still reach it.
  if (!section && [...params.keys()].length) return Promise.resolve(location.replace(`#/music/albums?${params}`));
  if ((!section || section === "home") && !id) return homePage(ctx);
  if (section === "albums") return id ? one && albumPage(id, ctx) : albumList(params, ctx);
  if (section === "artists") return id ? one && artistPage(id, ctx) : artistList(params, ctx);
  if (section === "songs" && !id) return songList(params, ctx);
  if (section === "genres") return id ? genrePage(decodeURIComponent(id), ctx) : genreList(ctx);
  if (section === "radio" && !id) return radioPage(ctx);
  if (section === "liked" && !id) return likedPage(ctx);
  if (section === "playlists") return id ? playlistPage(decodeURIComponent(id), ctx) : playlistsPage(params, ctx);
  if (section === "composers") return id ? one && composerPage(id, ctx) : composerList(params, ctx);
  if (section === "works") return id ? one && workPage(id, ctx) : workList(params, ctx);
  return null;
}

function artistItem(artist) {
  return h(
    "li",
    {},
    h(
      "a",
      { href: `#/music/artists/${artist.id}` },
      h("span", { class: "name" }, artist.name),
      h("span", { class: "count" }, artistCounts(artist)),
    ),
  );
}

const composerItem = (composer) =>
  countedItem(`#/music/composers/${composer.id}`, composer.name, plural(composer.works, "work", "works"));

async function albumList(params, ctx) {
  const filters = await genreFilter(ctx, "albums");
  if (!ctx.current()) return;
  await pagedList(
    { ...ctx, params },
    {
      title: "Albums",
      base: "#/music/albums",
      noun: ["album", "albums"],
      sorts: ALBUM_SORTS,
      defaultSort: "artist",
      path: "/music/albums",
      item: (album) => albumCard(album),
      list: h("ul", { class: "grid" }),
      filters,
    },
  );
}

async function artistList(params, ctx) {
  await pagedList(
    { ...ctx, params },
    {
      title: "Artists",
      base: "#/music/artists",
      noun: ["artist", "artists"],
      sorts: ARTIST_SORTS,
      defaultSort: "name",
      path: "/music/artists",
      item: artistItem,
      list: h("ul", { class: "artist-list" }),
    },
  );
}

async function composerList(params, ctx) {
  await pagedList(
    { ...ctx, params },
    {
      title: "Composers",
      base: "#/music/composers",
      noun: ["composer", "composers"],
      sorts: COMPOSER_SORTS,
      defaultSort: "name",
      path: "/music/composers",
      item: composerItem,
      list: h("ul", { class: "artist-list" }),
    },
  );
}

async function workList(params, ctx) {
  await pagedList(
    { ...ctx, params },
    {
      title: "Works",
      base: "#/music/works",
      noun: ["work", "works"],
      sorts: WORK_SORTS,
      defaultSort: "composer",
      path: "/music/works",
      item: (work) => workItem(work),
      list: h("ul", { class: "artist-list work-list" }),
    },
  );
}
