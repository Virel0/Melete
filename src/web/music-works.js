// A classical composer's page, and a work's, with its recordings side by side.

import { playQueue } from "/web/music-player.js";
import { api, clock, h, loading, metaRow, plural, show } from "/web/lib.js";
import { markPlaying, trackRow, workItem } from "/web/music-parts.js";

// ---------- A composer and a work ----------

export async function composerPage(id, ctx) {
  show("Composer", loading("the composer"));
  const composer = await api(`/music/composers/${id}`);
  if (!ctx.current()) return;
  show(
    composer.name,
    h("p", {}, h("a", { href: "#/music/composers" }, "All composers")),
    h("h1", {}, composer.name),
    h("p", { class: "status" }, plural(composer.works, "work", "works")),
    h("ul", { class: "artist-list work-list" }, composer.work_list.map((w) => workItem(w, { showComposer: false }))),
  );
}

// What one recording plays, as a queue: its tracks of the work, movement by movement.
function recordingQueue(recording) {
  return recording.tracks.map(({ track }) => ({
    id: track.id,
    title: track.title,
    artist: track.artist ?? recording.album.artist,
    album: recording.album.title,
    album_id: recording.album.id,
    cover: recording.album.cover,
    codec: track.codec,
    duration_seconds: track.duration_seconds,
  }));
}

// Who plays a recording and when: "Wiener Philharmoniker, 1975".
function recordingName(recording) {
  return [recording.artist ?? recording.album.artist, recording.album.year].filter(Boolean).join(", ");
}

// The work's movements down the side, its recordings across: each cell plays that recording
// from that movement, so the same movement can be heard one recording after another.
function comparison(work) {
  const header = h(
    "tr",
    {},
    h("th", { scope: "col" }, "Movement"),
    work.recording_list.map((recording) =>
      h(
        "th",
        { scope: "col" },
        h("a", { href: `#/music/albums/${recording.album.id}` }, recording.album.title),
        h("span", { class: "by" }, recordingName(recording)),
        h(
          "button",
          { type: "button", class: "quiet", onclick: () => playQueue(recordingQueue(recording), 0) },
          "Play",
          h("span", { class: "visually-hidden" }, ` ${recordingName(recording)}`),
        ),
      ),
    ),
  );
  const rows = work.movements.map((movement) =>
    h(
      "tr",
      {},
      h("th", { scope: "row" }, `${movement.number}. ${movement.name ?? ""}`.trim()),
      work.recording_list.map((recording) => {
        const at = recording.tracks.findIndex((t) => t.movement_number === movement.number);
        if (at < 0) return h("td", {}, h("span", { class: "missing" }, "Not on this album"));
        const { track } = recording.tracks[at];
        return h(
          "td",
          { "data-track": track.id },
          h(
            "button",
            { type: "button", class: "link track-play", onclick: () => playQueue(recordingQueue(recording), at) },
            track.duration_seconds ? clock(track.duration_seconds) : "Play",
            h("span", { class: "visually-hidden" }, ` ${movement.name ?? `movement ${movement.number}`}, ${recordingName(recording)}`),
          ),
        );
      }),
    ),
  );
  return h(
    "div",
    { class: "compare-wrap", tabindex: "0", role: "region", "aria-label": "Recordings side by side" },
    h("table", { class: "compare" }, h("thead", {}, header), h("tbody", {}, rows)),
  );
}

// A recording's tracks of the work, for a work without numbered movements.
function recordingSection(recording) {
  const queue = recordingQueue(recording);
  return h(
    "section",
    { class: "album-tracks" },
    h("h2", {}, h("a", { href: `#/music/albums/${recording.album.id}` }, recording.album.title)),
    h("p", { class: "note" }, recordingName(recording)),
    h(
      "ul",
      { class: "tracks" },
      recording.tracks.map(({ track, movement }, i) =>
        trackRow({ ...track, title: movement ?? track.title }, recording.album.artist, () => playQueue(queue, i)),
      ),
    ),
  );
}

export async function workPage(id, ctx) {
  show("Work", loading("the work"));
  const work = await api(`/music/works/${id}`);
  if (!ctx.current()) return;
  const body = work.movements.length ? comparison(work) : h("div", {}, work.recording_list.map(recordingSection));
  show(
    work.title,
    h("p", {}, h("a", { href: "#/music/works" }, "All works")),
    h("h1", {}, work.title),
    work.composer
      ? h("p", { class: "by-line" }, work.composer_id ? h("a", { href: `#/music/composers/${work.composer_id}` }, work.composer) : work.composer)
      : null,
    metaRow([
      plural(work.recordings, "recording", "recordings"),
      work.movements.length ? plural(work.movements.length, "movement", "movements") : null,
    ]),
    body,
  );
  markPlaying(body, ctx);
}
