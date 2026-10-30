// A track's actions, in a strip that opens under its row (Aoide's way: inline, not a floating
// menu): Play next, Play last, Add to playlist, Like, Start radio, and Go to its artists (and
// its album, where the row isn't on the album's page). One strip is open at a time; Escape
// closes it and focus goes back to the row's "More" button.
//
// trackActions(track, entry, { onAlbum }) gives { mark, more, strip } for a row to place: the
// heart shown while the track is liked, the button, and the strip. `track` is as the music API
// shows it (with `liked` and `artists`); `entry` is it as a queue entry.
//
// Likes live in Aoide's sync log, which is closed to someone who may see only some music
// libraries: that refusal (403) is said in a sentence in the strip.

import { SignedOut, api, h } from "/web/lib.js";
import { playLast, playNext } from "/web/music-player.js";
import { icon, iconButton } from "/web/music-controls.js";
import { playlistPicker } from "/web/music-playlist-pick.js";
import { radioButton } from "/web/music-radio.js";
import { hold, release } from "/web/page-parts.js";
import { SYNC_CLOSED, seesAllMusic } from "/web/music-access.js";

export const LIKES_CLOSED = "Likes aren't open to you, as you can see only some music libraries. An admin can change what you see.";

let made = 0;
// The strip open now: its close function.
let open = null;

// A like changed here: the track's id and whether it's liked, for other views to follow.
function likeChanged(id, liked) {
  document.dispatchEvent(new CustomEvent("melete:like", { detail: { id, liked } }));
}

function likeButton(track, mark, said) {
  const button = h("button", { type: "button", class: "quiet small like", "aria-pressed": String(Boolean(track.liked)) });
  const draw = () => {
    button.replaceChildren(icon(track.liked ? "liked" : "like"), "Like");
    button.setAttribute("aria-pressed", String(Boolean(track.liked)));
    mark.hidden = !track.liked;
  };
  button.addEventListener("click", async () => {
    if (!hold(button)) return;
    said.textContent = "";
    const want = !track.liked;
    try {
      await api(`/music/tracks/${track.id}/like`, { method: want ? "PUT" : "DELETE" });
      track.liked = want;
      draw();
      said.textContent = want ? `You like ${track.title}.` : `${track.title} is no longer liked.`;
      likeChanged(track.id, want);
    } catch (err) {
      if (err instanceof SignedOut) throw err;
      said.textContent = err.status === 403 ? LIKES_CLOSED : `Couldn't change it: ${err.message}`;
    } finally {
      release(button);
    }
  });
  draw();
  return button;
}

// Like for an album, on its page: an album's like is its own (`/music/albums/{id}/like`), and
// likes none of its tracks.
export function albumLike(album) {
  const said = h("span", { class: "note", role: "status" });
  const button = h("button", { type: "button", class: "quiet like" });
  const draw = () => {
    button.replaceChildren(icon(album.liked ? "liked" : "like"), "Like");
    button.setAttribute("aria-pressed", String(Boolean(album.liked)));
  };
  button.addEventListener("click", async () => {
    if (!hold(button)) return;
    said.textContent = "";
    const want = !album.liked;
    try {
      await api(`/music/albums/${album.id}/like`, { method: want ? "PUT" : "DELETE" });
      album.liked = want;
      draw();
      said.textContent = want ? `You like ${album.title}.` : `${album.title} is no longer liked.`;
    } catch (err) {
      if (err instanceof SignedOut) throw err;
      said.textContent = err.status === 403 ? LIKES_CLOSED : `Couldn't change it: ${err.message}`;
    } finally {
      release(button);
    }
  });
  draw();
  return [button, said];
}

// The strip's contents, made when it first opens.
function stripBody(track, entry, mark, onAlbum) {
  const said = h("p", { class: "note", role: "status" });
  const queued = (how, words) => () => {
    how([entry]);
    said.textContent = words;
  };
  const pickArea = h("div", { class: "pick-area" });
  const pick = h("button", { type: "button", class: "quiet small", "aria-expanded": "false" }, "Add to playlist");
  pick.addEventListener("click", () => {
    const opening = pick.getAttribute("aria-expanded") !== "true";
    pick.setAttribute("aria-expanded", String(opening));
    pickArea.replaceChildren(opening ? playlistPicker([track.id], track.title) : "");
  });
  const open = seesAllMusic();
  const [radio, radioSaid] = radioButton(async () => entry, track.title);
  radio?.classList.add("small");
  const artists = track.artists?.length ? track.artists : [];
  return [
    h(
      "div",
      { class: "strip-buttons" },
      h("button", { type: "button", class: "quiet small", onclick: queued(playNext, `${track.title} plays next.`) }, "Play next"),
      h("button", { type: "button", class: "quiet small", onclick: queued(playLast, `${track.title} is added to the end of Next up.`) }, "Play last"),
      open ? [pick, likeButton(track, mark, said), radio] : null,
      artists.map((a) => h("a", { class: "button quiet small", href: `#/music/artists/${a.id}` }, `Go to ${a.name}`)),
      onAlbum ? null : h("a", { class: "button quiet small", href: `#/music/albums/${track.album_id}` }, "Go to the album"),
    ),
    pickArea,
    said,
    open ? radioSaid : h("p", { class: "note" }, SYNC_CLOSED),
  ];
}

export function trackActions(track, entry, { onAlbum = false } = {}) {
  const id = `track-actions-${++made}`;
  const mark = h("span", { class: "liked-mark", hidden: !track.liked }, icon("liked"), h("span", { class: "visually-hidden" }, "Liked"));
  const strip = h("div", { class: "track-actions", id, role: "group", "aria-label": `Actions for ${track.title}`, hidden: true });
  const more = iconButton("more", `More for ${track.title}`, () => (strip.hidden ? show() : close()), "track-more");
  more.setAttribute("aria-expanded", "false");
  more.setAttribute("aria-controls", id);
  let built = false;
  function close(refocus = true) {
    strip.hidden = true;
    more.setAttribute("aria-expanded", "false");
    if (open === close) open = null;
    if (refocus) more.focus();
  }
  function show() {
    open?.(false);
    if (!built) strip.append(...stripBody(track, entry, mark, onAlbum).flat());
    built = true;
    strip.hidden = false;
    more.setAttribute("aria-expanded", "true");
    open = close;
    strip.querySelector("button, a")?.focus();
  }
  strip.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    event.stopPropagation();
    close();
  });
  return { mark, more, strip };
}
