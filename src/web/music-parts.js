// Pieces the music pages share: covers and album cards, a track's row (on an album, or a song
// from anywhere) and the mark on the one playing, and the Pin button.

import { nowPlaying } from "/web/music-player.js";
import { SignedOut, api, clock, h, plural } from "/web/lib.js";
import { hold, release } from "/web/page-parts.js";
import { cardLike } from "/web/music-album-likes.js";

// ---------- Pieces ----------

// The title is under the picture, so the picture says nothing to screen readers.
export function cover(src, extra = "") {
  return h("div", { class: `art square ${extra}`.trim() }, src ? h("img", { src, alt: "", loading: "lazy" }) : null);
}

export function albumCard(album, { showArtist = true } = {}) {
  return h(
    "li",
    { class: "album-card" },
    h(
      "a",
      { class: "card", href: `#/music/albums/${album.id}` },
      cover(album.cover),
      h("div", { class: "title", title: album.title }, album.title),
      showArtist ? h("div", { class: "meta clip", title: album.artist }, album.artist) : null,
      album.year ? h("div", { class: "meta" }, String(album.year)) : null,
    ),
    cardLike(album),
  );
}

// How much of an artist is here: "2 albums", "Appears on 3 albums", or both.
export function artistCounts(artist) {
  const own = artist.albums ? plural(artist.albums, "album", "albums") : "";
  const guest = artist.appears_on ? `appears on ${plural(artist.appears_on, "album", "albums")}` : "";
  if (!guest) return own || plural(0, "album", "albums");
  return own ? `${own} · ${guest}` : `A${guest.slice(1)}`;
}

// A composer, or a work: a name with how much of it is here.
export function countedItem(href, name, count) {
  return h(
    "li",
    {},
    h("a", { href }, h("span", { class: "name" }, name), h("span", { class: "count" }, count)),
  );
}

export function workItem(work, { showComposer = true } = {}) {
  const name = showComposer && work.composer ? `${work.composer}: ${work.title}` : work.title;
  return countedItem(`#/music/works/${work.id}`, name, plural(work.recordings, "recording", "recordings"));
}

export const SPATIAL = { atmos: "Dolby Atmos", multichannel: "Multichannel" };

// A row per track; its title plays the album from there. `actions`, when given, are its
// liked mark, More button and strip (music-actions.js).
export function trackRow(track, albumArtist, playFrom, actions = null) {
  return h(
    "li",
    { "data-track": track.id, class: actions ? "with-actions" : null },
    h("span", { class: "number" }, track.track_number ? String(track.track_number) : ""),
    h(
      "span",
      { class: "name" },
      h(
        "button",
        { type: "button", class: "link track-play", onclick: playFrom },
        h("span", { class: "visually-hidden" }, "Play "),
        track.title,
      ),
      actions ? [" ", actions.mark] : null,
      track.artist && track.artist !== albumArtist ? h("span", { class: "by" }, track.artist) : null,
      track.spatial ? [" ", h("span", { class: "badge" }, SPATIAL[track.spatial])] : null,
    ),
    h("span", { class: "length" }, track.duration_seconds ? clock(track.duration_seconds) : ""),
    actions ? [actions.more, actions.strip] : null,
  );
}

// A song from anywhere: its title plays the list it's in from there; its artist under it and
// its album beside it (under it on a phone). `actions` as for trackRow.
export function songRow(track, playFrom, actions = null) {
  return h(
    "li",
    { "data-track": track.id, class: actions ? "with-actions" : null },
    h(
      "span",
      { class: "name" },
      h(
        "button",
        { type: "button", class: "link track-play", onclick: playFrom },
        h("span", { class: "visually-hidden" }, "Play "),
        track.title,
      ),
      actions ? [" ", actions.mark] : null,
      h("span", { class: "by" }, track.artist ?? track.album_artist),
    ),
    h("a", { class: "album clip", href: `#/music/albums/${track.album_id}`, title: track.album }, track.album),
    h("span", { class: "length" }, track.duration_seconds ? clock(track.duration_seconds) : ""),
    actions ? [actions.more, actions.strip] : null,
  );
}

// Marks the track playing now, here and whenever it changes while the page is up.
export function markPlaying(root, ctx) {
  const mark = (id) => {
    for (const row of root.querySelectorAll("[data-track]")) {
      const here = String(id) === row.dataset.track;
      row.classList.toggle("playing", here);
      if (here) row.setAttribute("aria-current", "true");
      else row.removeAttribute("aria-current");
    }
  };
  const listener = (event) => mark(event.detail.id);
  document.addEventListener("melete:now-playing", listener);
  ctx.onLeave(() => document.removeEventListener("melete:now-playing", listener));
  mark(nowPlaying());
}

// ---------- Pins ----------

const MAX_PINS = 6;

// A Pin / Unpin button for an album or artist, with a line saying what happened. The
// pins are read when it's made and again on each press, so another device's changes count.
export function pinControl(kind, id, name) {
  const button = h("button", { type: "button", class: "quiet", "aria-disabled": "true" }, "Pin");
  const said = h("span", { class: "note", role: "status" });
  const key = String(id);
  const mine = (pins) => pins.items.some((p) => p.kind === kind && p.id === key);
  const label = (pinned) => {
    button.textContent = pinned ? "Unpin" : "Pin";
    button.setAttribute("aria-pressed", pinned ? "true" : "false");
    release(button);
  };
  api("/music/pins")
    .then((pins) => label(mine(pins)))
    .catch(() => (said.textContent = "Pins couldn't be read."));
  button.addEventListener("click", async () => {
    if (!hold(button)) return;
    try {
      const pins = await api("/music/pins");
      const items = pins.items.map(({ kind, id }) => ({ kind, id }));
      const pinned = mine(pins);
      if (!pinned && items.length >= MAX_PINS) {
        said.textContent = `You can pin up to ${MAX_PINS}. Unpin one from its page first.`;
        label(false);
        return;
      }
      const next = pinned ? items.filter((p) => !(p.kind === kind && p.id === key)) : [...items, { kind, id: key }];
      const saved = await api("/music/pins", { method: "PUT", body: { items: next } });
      document.dispatchEvent(new CustomEvent("melete:pins"));
      label(mine(saved));
      said.textContent = mine(saved) ? `${name} is pinned to Home.` : `${name} is unpinned.`;
    } catch (err) {
      if (err instanceof SignedOut) throw err;
      said.textContent = `Couldn't change your pins: ${err.message}`;
      release(button);
    }
  });
  return h("div", { class: "play-actions" }, button, said);
}
