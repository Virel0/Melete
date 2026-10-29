// Listen's Home, where `#/music` opens: Jump back in, Discover, Pinned, Recently added,
// Recently played, Most played lately, Made for you and Moods, Genres and In the house, in that
// order. A row with nothing in it is left out; a library with nothing in it says so in one
// sentence. Mixes, moods, the household and genres are extras: one that fails leaves its row out
// rather than the page.

import { SignedOut, api, h, number, plural, show } from "/web/lib.js";
import { clip, emptyState, hold, placeholders, release, unlessEmpty } from "/web/page-parts.js";
import { albumCard, cover, markPlaying, trackRow } from "/web/music-parts.js";
import { trackActions } from "/web/music-actions.js";
import { looseQueue, play, playAlbum, shufflePlay } from "/web/music-play.js";
import { genreHref } from "/web/music-browse.js";
import { pinnedRow } from "/web/music-pins.js";
import { seesAllMusic } from "/web/music-access.js";

const JUMP_BACK = 6;
const GENRES_SHOWN = 12;

function greeting() {
  const hour = new Date().getHours();
  if (hour < 5 || hour >= 18) return "Good evening";
  return hour < 12 ? "Good morning" : "Good afternoon";
}

// A row: its heading, then what's in it. Nothing in it, no row.
function row(key, title, body) {
  if (!body) return null;
  return h("section", { class: "home-row", "aria-labelledby": `home-${key}` }, h("h2", { id: `home-${key}` }, title), body);
}

// A line under a row that says what an action came to.
const statusLine = () => h("p", { class: "note", role: "status" });

// Runs a button's action: busy while it works, and a failure said in `said`.
async function busy(button, said, action) {
  if (!hold(button)) return;
  button.setAttribute("aria-busy", "true");
  said.textContent = "";
  try {
    await action();
  } catch (err) {
    if (err instanceof SignedOut) throw err;
    said.textContent = `Couldn't play it: ${err.message}`;
  } finally {
    release(button);
    button.removeAttribute("aria-busy");
  }
}

// The albums played last, each a card that plays the album at once.
function jumpBackIn(albums) {
  const said = statusLine();
  const cards = albums.slice(0, JUMP_BACK).map((album) => {
    const button = h(
      "button",
      { type: "button", class: "jump-card", "aria-label": `Play ${album.title} by ${album.artist}` },
      cover(album.cover),
      h("span", { class: "words" }, clip(album.title, "strong"), clip(album.artist)),
    );
    button.addEventListener("click", () => busy(button, said, () => playAlbum(album.id)));
    return h("li", {}, button);
  });
  return [h("ul", { class: "jump" }, cards), said];
}

// Tracks from different albums, numbered in order; each plays the list from there.
function looseTracks(tracks) {
  const queue = looseQueue(tracks);
  return h(
    "ul",
    { class: "tracks" },
    tracks.map((t, i) => trackRow({ ...t, track_number: i + 1 }, null, () => play(queue, i), trackActions(t, queue[i]))),
  );
}

// A mix as a wide card: what it is, Play and Shuffle, and its songs on request.
function mixCard(mix, heading) {
  const queue = looseQueue(mix.tracks);
  const name = h("span", { class: "visually-hidden" }, ` ${mix.name}`);
  return h(
    "div",
    { class: "wide-card" },
    h(
      "div",
      { class: "words" },
      heading ? h("h3", {}, mix.name) : h("strong", {}, `${plural(mix.tracks.length, "song", "songs")} for this week`),
      h("p", { class: "note" }, mix.about),
    ),
    h(
      "div",
      { class: "play-actions" },
      h("button", { type: "button", onclick: () => play(queue) }, "Play", name.cloneNode(true)),
      h("button", { type: "button", class: "quiet", onclick: () => shufflePlay(queue) }, "Shuffle", name),
    ),
    h("details", { class: "mix-tracks" }, h("summary", {}, `Show the ${plural(mix.tracks.length, "song", "songs")}`), looseTracks(mix.tracks)),
  );
}

// Ambient mixes chosen by how tracks sound. Playing one asks for its first hundred tracks.
function moods(list) {
  const shown = list.filter((m) => m.tracks > 0);
  if (!shown.length) return null;
  return h(
    "ul",
    { class: "moods" },
    shown.map((mix) => {
      const said = statusLine();
      const button = h("button", { type: "button", class: "quiet" }, "Play", h("span", { class: "visually-hidden" }, ` ${mix.name}`));
      button.addEventListener("click", () =>
        busy(button, said, async () => {
          const next = await api(`/music/mixes/${mix.key}/next`, { method: "POST", body: { queue: [], limit: 100 } });
          if (next.tracks.length) play(looseQueue(next.tracks));
          else said.textContent = "Nothing fits it right now.";
        }),
      );
      return h("li", {}, h("h3", {}, mix.name), h("p", { class: "note" }, `${mix.about} ${plural(mix.tracks, "track fits", "tracks fit")}.`), h("div", { class: "play-actions" }, button, said));
    }),
  );
}

function genreChips(genres) {
  return h(
    "ul",
    { class: "pills" },
    genres.slice(0, GENRES_SHOWN).map((g) =>
      h(
        "li",
        {},
        h("a", { href: genreHref(g.name) }, g.name, h("span", { class: "count" }, number.format(g.albums))),
      ),
    ),
  );
}

// What the household is listening to: each person chooses for themselves whether to share
// theirs, off until they turn it on; it never leaves the server. Who shares is listed only when
// someone does; the choice is always there.
function household(house) {
  const box = h("input", { type: "checkbox", id: "share-listening", checked: house.sharing });
  const said = statusLine();
  box.addEventListener("change", async () => {
    box.disabled = true;
    try {
      const now = await api("/music/household/sharing", { method: "PUT", body: { sharing: box.checked } });
      box.checked = now.sharing;
      said.textContent = now.sharing
        ? "Shared: the household sees what you play now and played in the last week."
        : "Not shared: the household sees nothing of yours from now on.";
    } catch (err) {
      if (err instanceof SignedOut) throw err;
      box.checked = !box.checked;
      said.textContent = `Couldn't change it: ${err.message}`;
    } finally {
      box.disabled = false;
    }
  });
  const by = (t) => t.artist ?? t.album_artist;
  const people = house.people.map((p) => {
    const now = p.now_playing;
    const heard = p.recently_played.slice(0, 3).map((x) => `${x.track.title} by ${by(x.track)}`);
    return h(
      "li",
      {},
      h(
        "div",
        {},
        h("strong", {}, p.user),
        now
          ? h("p", {}, `Playing ${now.track.title} by ${by(now.track)}`, now.track.album_id ? [" from ", h("a", { href: `#/music/albums/${now.track.album_id}` }, now.track.album)] : null, ".")
          : h("p", { class: "note" }, "Not playing anything now."),
        heard.length ? h("p", { class: "note" }, `Lately: ${heard.join("; ")}.`) : null,
      ),
    );
  });
  return [
    people.length ? h("ul", { class: "rows" }, people) : null,
    h("div", { class: "check" }, box, h("label", { for: "share-listening" }, "Share what I listen to with the household")),
    said,
  ];
}

// The rows, in order, each left out when it has nothing.
function rows(home, pins, mixList, house, genres) {
  const discover = home.mixes.find((m) => m.key === "discover");
  const others = home.mixes.filter((m) => m !== discover && m.tracks.length);
  const shelf = (albums) => unlessEmpty(albums, (list) => h("ul", { class: "grid shelf" }, list.map((a) => albumCard(a))));
  const madeForYou = others.length ? h("div", { class: "wide-cards" }, others.map((m) => mixCard(m, true))) : null;
  return [
    row("jump", "Jump back in", unlessEmpty(home.recently_played, jumpBackIn)),
    row("discover", "Discover", discover?.tracks.length ? mixCard(discover, false) : null),
    row("pinned", "Pinned", unlessEmpty(pins.items, () => pinnedRow(pins))),
    row("added", "Recently added", shelf(home.recently_added)),
    row("played", "Recently played", shelf(home.recently_played)),
    row("most", "Most played lately", unlessEmpty(home.most_played, (list) => looseTracks(list.map((m) => m.track)))),
    row("for-you", "Made for you", madeForYou),
    row("moods", "Moods", moods(mixList)),
    row("genres", "Genres", unlessEmpty(genres, genreChips)),
    house ? row("house", "In the house", household(house)) : null,
  ];
}

export async function homePage(ctx) {
  const title = h("h1", {}, greeting());
  show("Listen", title, placeholders("your music", { kind: "cards" }));
  const quiet = (promise, empty) => promise.catch((err) => (err instanceof SignedOut ? Promise.reject(err) : empty));
  const [home, pins, mixList, house, genres] = await Promise.all([
    api("/music/home"),
    quiet(api("/music/pins"), { items: [] }),
    quiet(api("/music/mixes"), []),
    seesAllMusic() ? quiet(api("/music/household"), null) : null,
    quiet(api("/music/genres"), []),
  ]);
  if (!ctx.current()) return;
  if (!home.recently_added.length && !home.recently_played.length) {
    show("Listen", title, emptyState("Nothing in the music library yet. Albums show here once a music library is scanned."));
    return;
  }
  const root = h("div", { class: "listen-home" }, rows(home, pins, mixList, house, genres));
  show("Listen", title, root);
  markPlaying(root, ctx);
}
