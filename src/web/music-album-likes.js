// The heart on an album card (music-parts.js): a toggle that likes and unlikes the album as its
// page's Like does (music-actions.js), its own like (`/music/albums/{id}/like`), liking none of
// its tracks. Cards learn which albums are liked from `GET /music/likes`, kept for a minute so
// a like set on a device shows soon, changed at once when a like changes here (on a card or the
// album's page, which redraws every card of that album), and dropped when signing out.
//
// Likes live in Aoide's sync log, which is closed to someone who may see only some music
// libraries: they get no hearts. A heart is shown once the likes are known, so it never says
// "not liked" of a liked album; when they can't be had, the cards go without.

import { SignedOut, api, h } from "/web/lib.js";
import { icon } from "/web/music-controls.js";
import { hold, release } from "/web/page-parts.js";
import { LIKES_CLOSED, seesAllMusic } from "/web/music-access.js";

const FRESH = 60_000;
let kept = null;
let keptAt = 0;

// The ids of the albums the listener likes, as kept or asked for afresh.
function likedAlbums() {
  if (!kept || Date.now() - keptAt > FRESH) {
    keptAt = Date.now();
    kept = api("/music/likes")
      .then((likes) => new Set(likes.albums.filter((l) => l.album).map((l) => l.album.id)))
      .catch((err) => {
        kept = null;
        throw err;
      });
  }
  return kept;
}

function draw(button, liked) {
  button.replaceChildren(icon(liked ? "liked" : "like"));
  button.setAttribute("aria-pressed", String(liked));
}

// A like changed here: kept, and every card of the album on the page follows.
export function albumLikeChanged(id, liked) {
  kept?.then((ids) => (liked ? ids.add(id) : ids.delete(id)), () => {});
  for (const button of document.querySelectorAll(`button.card-like[data-album="${id}"]`)) draw(button, liked);
}

// Signing out forgets them, so whoever signs in next in this tab sees only their own.
export function forgetAlbumLikes() {
  kept = null;
}

// The card's heart and the sentence that says when a change didn't work; null for someone to
// whom likes are closed.
export function cardLike(album) {
  if (!seesAllMusic()) return null;
  const said = h("span", { class: "note", role: "status" });
  const button = h("button", { type: "button", class: "icon card-like", "data-album": album.id, "aria-label": `Like ${album.title}`, title: "Like", hidden: true });
  button.addEventListener("click", async () => {
    if (!hold(button)) return;
    said.textContent = "";
    const want = button.getAttribute("aria-pressed") !== "true";
    try {
      await api(`/music/albums/${album.id}/like`, { method: want ? "PUT" : "DELETE" });
      albumLikeChanged(album.id, want);
    } catch (err) {
      if (err instanceof SignedOut) throw err;
      said.textContent = err.status === 403 ? LIKES_CLOSED : `Couldn't change it: ${err.message}`;
    } finally {
      release(button);
    }
  });
  likedAlbums().then(
    (ids) => {
      draw(button, ids.has(album.id));
      button.hidden = false;
    },
    () => {},
  );
  return [button, said];
}
