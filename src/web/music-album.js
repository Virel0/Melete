// An artist's page, with their albums and the ones they appear on, and an album's: its cover on
// the album's own colours, what its files are, Play, Shuffle, Start radio, Like, Add to playlist
// and Pin, then its tracks by disc, each with its actions (music-actions.js).

import { SignedOut, api, h, loading, metaRow, number, plural, runtime, show } from "/web/lib.js";
import { albumLike, trackActions } from "/web/music-actions.js";
import { SPATIAL, albumCard, artistCounts, markPlaying, pinControl, trackRow } from "/web/music-parts.js";
import { albumQueue, looseQueue, play, shufflePlay } from "/web/music-play.js";
import { playlistPicker } from "/web/music-playlist-pick.js";
import { RADIO_CLOSED, albumSeed, artistSeed, radioButton } from "/web/music-radio.js";
import { hold, release } from "/web/page-parts.js";
import { SYNC_CLOSED, seesAllMusic } from "/web/music-access.js";

// Play, Shuffle and Start radio for an artist: their songs (their albums', and the ones
// crediting them elsewhere), asked for when pressed.
function artistActions(artist) {
  const said = h("span", { class: "note", role: "status" });
  const start = (how) => async (event) => {
    const button = event.currentTarget;
    if (!hold(button)) return;
    said.textContent = "";
    try {
      const page = await api(`/music/tracks?${new URLSearchParams({ artist_id: String(artist.id), limit: "500" })}`);
      how(looseQueue(page.items));
    } catch (err) {
      if (err instanceof SignedOut) throw err;
      said.textContent = `Couldn't play them: ${err.message}`;
    } finally {
      release(button);
    }
  };
  const hidden = h("span", { class: "visually-hidden" }, ` ${artist.name}`);
  return h(
    "div",
    { class: "play-actions" },
    h("button", { type: "button", onclick: start((q) => play(q)) }, "Play", hidden.cloneNode(true)),
    h("button", { type: "button", class: "quiet", onclick: start(shufflePlay) }, "Shuffle", hidden),
    radioButton(() => artistSeed(artist.id), artist.name),
    said,
    seesAllMusic() ? null : h("p", { class: "note" }, RADIO_CLOSED),
  );
}

// ---------- An artist ----------

export async function artistPage(id, ctx) {
  show("Artist", loading("the artist"));
  const artist = await api(`/music/artists/${id}`);
  if (!ctx.current()) return;
  const guest = artist.appears_on_list;
  show(
    artist.name,
    h("p", {}, h("a", { href: "#/music/artists" }, "All artists")),
    h("h1", {}, artist.name),
    h("p", { class: "status" }, artistCounts(artist)),
    artistActions(artist),
    pinControl("artist", artist.id, artist.name),
    artist.album_list.length
      ? h("ul", { class: "grid" }, artist.album_list.map((album) => albumCard(album, { showArtist: false })))
      : guest.length
        ? null
        : h("p", { class: "status" }, "None of this artist's albums are in the library right now."),
    guest.length
      ? h(
          "section",
          { class: "appears-on" },
          h("h2", {}, "Appears on"),
          h("ul", { class: "grid" }, guest.map((album) => albumCard(album))),
        )
      : null,
  );
}

// ---------- An album ----------

const CODECS = {
  flac: "FLAC",
  alac: "ALAC",
  mp3: "MP3",
  aac: "AAC",
  opus: "Opus",
  vorbis: "Vorbis",
  wav: "WAV",
  aiff: "AIFF",
  wavpack: "WavPack",
  ape: "APE",
  musepack: "Musepack",
  speex: "Speex",
};

// What the album's files are, when they all agree: "FLAC, 24-bit, 96 kHz" or "MP3, 320 kbit/s".
function quality(tracks) {
  const one = (pick) => {
    const values = new Set(tracks.map(pick));
    return values.size === 1 ? [...values][0] : null;
  };
  const codec = one((t) => t.codec);
  if (!codec) return null;
  const depth = one((t) => t.bit_depth);
  const rate = one((t) => t.sample_rate);
  const bitrate = one((t) => t.bitrate_kbps);
  const lossless = ["flac", "alac", "wav", "aiff", "wavpack", "ape"].includes(codec);
  const parts = [CODECS[codec] ?? codec.toUpperCase()];
  if (lossless) {
    if (depth) parts.push(`${depth}-bit`);
    if (rate) parts.push(`${number.format(rate / 1000)} kHz`);
  } else if (bitrate) {
    parts.push(`${number.format(bitrate)} kbit/s`);
  }
  return parts.join(", ");
}

// What an album's spatial audio is: every track the same, or how many tracks have it.
function spatialLine(tracks) {
  const kinds = new Set(tracks.map((t) => t.spatial ?? null));
  if (kinds.size === 1 && !kinds.has(null)) return SPATIAL[[...kinds][0]];
  const atmos = tracks.filter((t) => t.spatial === "atmos").length;
  if (atmos) return `Dolby Atmos on ${plural(atmos, "track", "tracks")}`;
  const multi = tracks.filter((t) => t.spatial === "multichannel").length;
  return multi ? `Multichannel on ${plural(multi, "track", "tracks")}` : null;
}

// Tracks by disc; a heading per disc only when there's more than one.
function trackList(tracks, albumArtist, queue) {
  const discs = new Map();
  for (const track of tracks) {
    const disc = track.disc_number ?? 1;
    if (!discs.has(disc)) discs.set(disc, []);
    discs.get(disc).push(track);
  }
  const many = discs.size > 1;
  return [...discs].map(([disc, list]) =>
    h(
      "section",
      { class: "disc" },
      many ? h("h3", {}, `Disc ${disc}`) : null,
      h(
        "ul",
        { class: "tracks" },
        list.map((t) => {
          const at = tracks.indexOf(t);
          return trackRow(t, albumArtist, () => play(queue, at), trackActions(t, queue[at], { onAlbum: true }));
        }),
      ),
    ),
  );
}

function matchNote(status) {
  if (status === "ambiguous") {
    return h("p", { class: "note" }, "MusicBrainz has more than one album this could be. An admin can pick the right one.");
  }
  if (status === "unmatched") return h("p", { class: "note" }, "Not found on MusicBrainz.");
  return null;
}

// The album's colours from its cover, for its head: the background and the text that reads on
// it (at least 4.5:1, worked out on the server). Set through the style object, which the page's
// policy allows where a style attribute isn't.
function paint(head, colours) {
  if (!colours) return;
  head.classList.add("coloured");
  head.style.setProperty("--album-bg", colours.background);
  head.style.setProperty("--album-ink", colours.text1);
  head.style.setProperty("--album-line", colours.text2);
}

// Play, Shuffle, Start radio, Add to playlist and Pin for the whole album.
function albumActions(album, queue) {
  const area = h("div", { class: "pick-area" });
  const pick = h("button", { type: "button", class: "quiet", "aria-expanded": "false" }, "Add to playlist");
  pick.addEventListener("click", () => {
    const opening = pick.getAttribute("aria-expanded") !== "true";
    pick.setAttribute("aria-expanded", String(opening));
    area.replaceChildren(opening ? playlistPicker(album.track_list.map((t) => t.id), album.title) : "");
  });
  return [
    h(
      "div",
      { class: "play-actions" },
      h("button", { type: "button", onclick: () => play(queue) }, "Play"),
      h("button", { type: "button", class: "quiet", onclick: () => shufflePlay(queue) }, "Shuffle"),
      radioButton(() => albumSeed(album.id), album.title),
      seesAllMusic() ? albumLike(album) : null,
      seesAllMusic() ? pick : null,
      pinControl("album", album.id, album.title),
    ),
    seesAllMusic() ? area : h("p", { class: "note" }, SYNC_CLOSED),
  ];
}

function albumHead(album, queue) {
  const minutes = album.duration_seconds ? Math.round(album.duration_seconds / 60) : null;
  const sound = [quality(album.track_list), spatialLine(album.track_list)].filter(Boolean);
  const head = h(
    "div",
    { class: "album-head" },
    h("div", { class: "album-cover" }, album.cover ? h("img", { src: album.cover, alt: "" }) : null),
    h(
      "div",
      { class: "facts" },
      h("p", { class: "kind" }, album.compilation ? "Compilation" : "Album"),
      h("h1", {}, album.title),
      h("p", { class: "by-line" }, album.artist_id ? h("a", { href: `#/music/artists/${album.artist_id}` }, album.artist) : album.artist),
      metaRow([album.year ? String(album.year) : null, plural(album.tracks, "song", "songs"), runtime(minutes), album.genres.join(", ") || null]),
      sound.length ? metaRow(sound) : null,
      album.track_list.length ? albumActions(album, queue) : pinControl("album", album.id, album.title),
      matchNote(album.match_status),
    ),
  );
  paint(head, album.colours);
  return head;
}

export async function albumPage(id, ctx) {
  show("Album", loading("the album"));
  const album = await api(`/music/albums/${id}`);
  if (!ctx.current()) return;
  ctx.tint(album.cover, album.colours?.background);
  const queue = albumQueue(album);
  const tracks = h(
    "section",
    { class: "album-tracks" },
    h("h2", { class: "visually-hidden" }, "Tracks"),
    album.track_list.length ? trackList(album.track_list, album.artist, queue) : h("p", { class: "status" }, "No tracks in the library right now."),
  );
  show(album.title, albumHead(album, queue), tracks);
  markPlaying(tracks, ctx);
}
