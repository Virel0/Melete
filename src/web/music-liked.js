// Liked songs and Liked albums: the tracks and albums this listener likes, newest like first,
// from the same likes their Aoide devices keep (`GET /music/likes`); songs with Play and
// Shuffle, albums as cards. A liked track or album gone from the library stays, greyed, with
// what it was.
//
// Likes live in Aoide's sync log, which is closed to someone who may see only some music
// libraries: that refusal (403) is the page's one sentence.

import { api, h, plural, show } from "/web/lib.js";
import { LIKES_CLOSED, trackActions } from "/web/music-actions.js";
import { albumCard, markPlaying, songRow } from "/web/music-parts.js";
import { looseQueue, play, shufflePlay } from "/web/music-play.js";
import { emptyState, placeholders } from "/web/page-parts.js";
import { seesAllMusic } from "/web/music-access.js";

// "artist|album|title|seconds", as the apps key a track: what a gone track was.
export function goneRow(contentKey, tools = null) {
  const [artist, album, title] = (contentKey ?? "").split("|");
  return h(
    "li",
    { class: "gone" },
    h("span", { class: "name" }, h("span", {}, title || "A song"), h("span", { class: "by" }, [artist, album].filter(Boolean).join(", "))),
    h("span", { class: "missing" }, "Not in the library"),
    tools,
  );
}

export async function likedPage(ctx) {
  const title = h("h1", {}, "Liked songs");
  if (!seesAllMusic()) return show("Liked songs", title, emptyState(LIKES_CLOSED));
  show("Liked songs", title, placeholders("your liked songs"));
  let likes;
  try {
    likes = await api("/music/likes");
  } catch (err) {
    if (err.status !== 403) throw err;
    if (ctx.current()) show("Liked songs", title, emptyState(LIKES_CLOSED));
    return;
  }
  if (!ctx.current()) return;
  if (!likes.tracks.length) {
    show("Liked songs", title, emptyState("Songs you like show here. Choose More beside any song, then Like.", { href: "#/music/songs", label: "Go to Songs" }));
    return;
  }
  const here = likes.tracks.filter((l) => l.track).map((l) => l.track);
  const queue = looseQueue(here);
  const list = h(
    "ul",
    { class: "tracks songs" },
    likes.tracks.map((l) => {
      if (!l.track) return goneRow(l.content_key);
      const at = here.indexOf(l.track);
      return songRow(l.track, () => play(queue, at), trackActions(l.track, queue[at]));
    }),
  );
  const gone = likes.tracks.length - here.length;
  show(
    "Liked songs",
    title,
    h("p", { class: "status" }, plural(likes.tracks.length, "song", "songs"), gone ? `, ${gone} of them no longer in the library` : ""),
    here.length
      ? h(
          "div",
          { class: "play-actions" },
          h("button", { type: "button", onclick: () => play(queue) }, "Play"),
          h("button", { type: "button", class: "quiet", onclick: () => shufflePlay(queue) }, "Shuffle"),
        )
      : null,
    list,
  );
  markPlaying(list, ctx);
}

export async function likedAlbumsPage(ctx) {
  const title = h("h1", {}, "Liked albums");
  if (!seesAllMusic()) return show("Liked albums", title, emptyState(LIKES_CLOSED));
  show("Liked albums", title, placeholders("your liked albums"));
  let likes;
  try {
    likes = await api("/music/likes");
  } catch (err) {
    if (err.status !== 403) throw err;
    if (ctx.current()) show("Liked albums", title, emptyState(LIKES_CLOSED));
    return;
  }
  if (!ctx.current()) return;
  if (!likes.albums.length) {
    show("Liked albums", title, emptyState("Albums you like show here. Open an album, then choose Like.", { href: "#/music/albums", label: "Go to Albums" }));
    return;
  }
  const here = likes.albums.filter((l) => l.album);
  const gone = likes.albums.filter((l) => !l.album);
  show(
    "Liked albums",
    title,
    h("p", { class: "status" }, plural(likes.albums.length, "album", "albums"), gone.length ? `, ${gone.length} of them no longer in the library` : ""),
    here.length ? h("ul", { class: "grid" }, here.map((l) => albumCard(l.album))) : null,
    gone.length
      ? h(
          "ul",
          { class: "tracks songs" },
          gone.map((l) => {
            // "artist|album", as the apps key an album: what a gone album was.
            const [artist, album] = (l.content_key ?? "").split("|");
            return h("li", { class: "gone" }, h("span", { class: "name" }, h("span", {}, album || "An album"), h("span", { class: "by" }, artist || "")), h("span", { class: "missing" }, "Not in the library"));
          }),
        )
      : null,
  );
}
