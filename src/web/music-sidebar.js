// What Listen's sidebar lists under its sections: Yours (Liked songs, Liked albums and
// Playlists), Pinned (the albums, artists and playlists pinned on any device, in their order),
// Collections (the listener's saved song filters, music-collections.js) and Playlists (the
// listener's own and those shared with them, a shared one named with who shares it). sides.js
// asks for them as the sidebar's `more` groups.
//
// The answers are kept for a minute, and dropped at once when a page here changes the pins or
// a playlist (`melete:pins`, `melete:playlists`), so the next sidebar drawn shows the change, and
// when signing out. Playlists aren't asked for at all for a listener to whom they're closed (a
// user who may see only some music libraries, as /me says).

import { api } from "/web/lib.js";
import { seesAllMusic } from "/web/music-access.js";
import { playlistHref } from "/web/music-playlists.js";
import { collectionHref } from "/web/music-collections.js";

const FRESH = 60_000;
let kept = null;
let keptAt = 0;
async function load() {
  const closed = !seesAllMusic();
  const [pins, playlists, collections] = await Promise.all([
    api("/music/pins").catch(() => ({ items: [] })),
    closed ? [] : api("/playlists").catch(() => []),
    api("/music/collections").catch(() => ({ items: [] })),
  ]);
  return { pins: pins.items, playlists, collections: collections.items, closed };
}

// The pins and playlists, as kept or asked for afresh.
export function listenLists() {
  if (!kept || Date.now() - keptAt > FRESH) {
    keptAt = Date.now();
    kept = load().catch((err) => {
      kept = null;
      throw err;
    });
  }
  return kept;
}

const forget = () => (kept = null);
document.addEventListener("melete:pins", forget);
document.addEventListener("melete:playlists", forget);
document.addEventListener("melete:collections", forget);

// Signing out forgets them, so whoever signs in next in this tab sees only their own.
export function forgetListenLists() {
  kept = null;
}

// Where a pin leads and what it's called; null for one gone from the library.
function pinItem(pin) {
  if (pin.album) return { href: `#/music/albums/${pin.album.id}`, name: pin.album.title };
  if (pin.artist) return { href: `#/music/artists/${pin.artist.id}`, name: pin.artist.name };
  if (pin.playlist) return { href: playlistHref(pin.playlist.id), name: pin.playlist.name };
  return null;
}

// The sidebar's Yours (the listener's own pages, `yours`), Pinned, Collections and Playlists
// groups for the page at `hash`; a group with nothing in it is left out, and Yours when
// playlists are closed.
export function listenGroups({ pins, playlists, collections = [], closed }, hash, yours) {
  const mark = (item) => ({ ...item, here: item.href === hash });
  const pinned = pins.map(pinItem).filter(Boolean).map(mark);
  const lists = playlists.map((p) => mark({ href: playlistHref(p.id), name: p.mine ? p.name : `Shared by ${p.owner}: ${p.name}` }));
  const saved = collections.map((c) => mark({ href: collectionHref(c), name: c.name }));
  return [
    closed ? null : { name: "Yours", items: yours },
    pinned.length ? { name: "Pinned", items: pinned } : null,
    saved.length ? { name: "Collections", items: saved } : null,
    lists.length ? { name: "Playlists", items: lists } : null,
  ].filter(Boolean);
}
