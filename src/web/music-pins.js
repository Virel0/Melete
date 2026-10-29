// Home's Pinned row: the albums, artists and playlists pinned on any device, in their order, each
// with Unpin. One that's gone from the library says so.

import { SignedOut, api, h } from "/web/lib.js";
import { albumCard, cover } from "/web/music-parts.js";
import { playlistHref } from "/web/music-playlists.js";

function pinItem(pin, unpin) {
  const name = pin.album?.title ?? pin.artist?.name ?? pin.playlist?.name ?? `a pinned ${pin.kind}`;
  const off = h("button", { type: "button", class: "quiet small", onclick: () => unpin(pin), "aria-label": `Unpin ${name}` }, "Unpin");
  if (pin.album) {
    const card = albumCard(pin.album);
    card.append(off);
    return card;
  }
  const link = pin.artist
    ? h("a", { class: "card", href: `#/music/artists/${pin.artist.id}` }, cover(null), h("div", { class: "title" }, pin.artist.name), h("div", { class: "meta" }, "Artist"))
    : pin.playlist
      ? h("a", { class: "card", href: playlistHref(pin.playlist.id) }, cover(pin.playlist.image_url), h("div", { class: "title" }, pin.playlist.name), h("div", { class: "meta" }, "Playlist"))
      : h("div", { class: "card" }, cover(null), h("div", { class: "title" }, "No longer here"), h("div", { class: "meta" }, `A pinned ${pin.kind}`));
  return h("li", {}, link, off);
}

// The pinned row, with unpinning in place.
export function pinnedRow(pins) {
  const grid = h("ul", { class: "grid shelf" });
  const said = h("p", { class: "note", role: "status" });
  const draw = (items) => grid.replaceChildren(...items.map((pin) => pinItem(pin, unpin)));
  const unpin = async (pin) => {
    try {
      // Read again first, so a change made on another device isn't undone.
      const now = await api("/music/pins");
      const items = now.items.filter((p) => !(p.kind === pin.kind && p.id === pin.id)).map(({ kind, id }) => ({ kind, id }));
      const saved = await api("/music/pins", { method: "PUT", body: { items } });
      document.dispatchEvent(new CustomEvent("melete:pins"));
      draw(saved.items);
      said.textContent = saved.items.length ? "Unpinned." : "Unpinned. Pin albums and artists from their pages.";
      grid.querySelector("button")?.focus();
    } catch (err) {
      if (err instanceof SignedOut) throw err;
      said.textContent = `Couldn't unpin it: ${err.message}`;
    }
  };
  draw(pins.items);
  return [grid, said];
}
