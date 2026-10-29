// Radio: songs like the one it starts from, chosen from the whole library by what this
// listener plays (`POST /aoide/next`), played after it. Started from an album, an artist or a
// song (radioButton), or from the Radio page (what's playing, or an album played lately).
//
// Radio reads the listener's sync log, which is closed to someone who may see only some music
// libraries: that refusal (403) is said in a sentence where the button is.

import { SignedOut, api, h, show } from "/web/lib.js";
import { emptyState, hold, placeholders, release } from "/web/page-parts.js";
import * as queue from "/web/music-queue.js";
import { albumQueue, looseQueue, play } from "/web/music-play.js";
import { cover } from "/web/music-parts.js";
import { seesAllMusic } from "/web/music-access.js";

const SONGS = 50;
const RECENT = 40;

export const RADIO_CLOSED = "Radio isn't open to you, as you can see only some music libraries. An admin can change what you see.";

// What this tab played lately, newest first, so radio doesn't play it again at once.
function recentlyPlayed() {
  const all = queue.all();
  return all
    .filter((_, i) => queue.partOf(i) === "played")
    .map((e) => String(e.track.id))
    .reverse()
    .slice(0, RECENT);
}

// Plays radio from `seed` (a queue entry): the seed, then what the server chose after it, in
// its order. Answers how many songs follow the seed.
export async function startRadio(seed) {
  const next = await api("/aoide/next", { method: "POST", body: { seed: String(seed.id), recent: recentlyPlayed(), limit: SONGS } });
  const ids = next.candidates.map((c) => c.id);
  if (!ids.length) {
    play([seed]);
    return 0;
  }
  const found = await api(`/music/tracks?${new URLSearchParams({ id: ids.join(","), limit: String(ids.length) })}`);
  const byId = new Map(found.items.map((t) => [String(t.id), t]));
  const chosen = ids.map((id) => byId.get(id)).filter(Boolean);
  play([seed, ...looseQueue(chosen)]);
  return chosen.length;
}

// What went wrong starting radio, in a sentence.
export function radioTrouble(err) {
  if (err.status === 403) return RADIO_CLOSED;
  if (err.status === 404) return "That song isn't in the library now.";
  return `Couldn't start radio: ${err.message}`;
}

// A Start radio button, with a line saying how it went. `seed()` gives the queue entry to start
// from (it may ask the server); `name` is what radio is from, for the button's label. Nothing
// for someone to whom radio is closed: the page says so instead.
export function radioButton(seed, name, { quiet = true } = {}) {
  if (!seesAllMusic()) return [];
  const said = h("span", { class: "note", role: "status" });
  const button = h(
    "button",
    { type: "button", class: quiet ? "quiet" : null },
    "Start radio",
    name ? h("span", { class: "visually-hidden" }, ` from ${name}`) : null,
  );
  button.addEventListener("click", async () => {
    if (!hold(button)) return;
    button.setAttribute("aria-busy", "true");
    said.textContent = "";
    try {
      const from = await seed();
      if (!from) {
        said.textContent = "There's nothing here to start radio from.";
        return;
      }
      const count = await startRadio(from);
      said.textContent = count ? `Radio from ${from.title}: ${count} songs after it.` : "Nothing else fits yet: playing it on its own.";
    } catch (err) {
      if (err instanceof SignedOut) throw err;
      said.textContent = radioTrouble(err);
    } finally {
      release(button);
      button.removeAttribute("aria-busy");
    }
  });
  return [button, said];
}

// An album's first track, as a seed.
export async function albumSeed(id) {
  const album = await api(`/music/albums/${id}`);
  return albumQueue(album)[0] ?? null;
}

// One of an artist's tracks, chosen at random, as a seed.
export async function artistSeed(id) {
  const page = await api(`/music/tracks?${new URLSearchParams({ artist_id: String(id), limit: "100" })}`);
  const tracks = looseQueue(page.items);
  return tracks[Math.floor(Math.random() * tracks.length)] ?? null;
}

// The Radio page: start from what's playing, or from an album played lately.
export async function radioPage(ctx) {
  const title = h("h1", {}, "Radio");
  const about = h(
    "p",
    { class: "lead" },
    "Radio plays songs like the one it starts from, chosen from the whole library by what you listen to. Start it from any album, artist or song, or from here.",
  );
  if (!seesAllMusic()) return show("Radio", title, emptyState(RADIO_CLOSED));
  show("Radio", title, about, placeholders("albums you played lately", { kind: "rows", count: 4 }));
  const home = await api("/music/home");
  if (!ctx.current()) return;
  const playing = queue.current();
  const now = playing
    ? h(
        "section",
        { class: "listen-row radio-now", "aria-labelledby": "radio-now" },
        h("h2", { id: "radio-now" }, "From what's playing"),
        h("p", {}, `${playing.title}${playing.artist ? ` by ${playing.artist}` : ""}`),
        h("div", { class: "play-actions" }, radioButton(async () => playing, playing.title, { quiet: false })),
      )
    : null;
  const albums = home.recently_played.map((album) =>
    h(
      "li",
      {},
      cover(album.cover),
      h("div", { class: "words" }, h("a", { href: `#/music/albums/${album.id}` }, album.title), h("span", { class: "by" }, album.artist)),
      h("div", { class: "play-actions" }, radioButton(() => albumSeed(album.id), album.title)),
    ),
  );
  show(
    "Radio",
    title,
    about,
    now,
    albums.length
      ? h("section", { class: "listen-row", "aria-labelledby": "radio-albums" }, h("h2", { id: "radio-albums" }, "From an album you played lately"), h("ul", { class: "radio-seeds" }, albums))
      : now
        ? null
        : emptyState("Play something first, or start radio from any album, artist or song.", { href: "#/music/albums", label: "Browse albums" }),
  );
}
