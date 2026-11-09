// A playlist file read as the server reads it (src/playlist_import/reading.rs: RFC 4180 rows,
// columns found by the names exporters give them), and what the library has of it, shown
// before anything is saved: the tracks matched by `POST /aoide/match`, by the rules Aoide's
// apps share, so the web, the server and a phone agree on what's missing.

import { SignedOut, api, h, number, plural } from "/web/lib.js";

const TITLE = ["track name", "title", "track", "name", "song"];
const ARTIST = ["artist name(s)", "artist name", "artist", "artists", "artist(s)"];
const ALBUM = ["album name", "album"];
const LENGTH = ["track duration (ms)", "duration (ms)", "duration_ms", "duration"];
const ISRC = ["isrc"];
// The most tracks one import takes, and one match call.
const MAX_TRACKS = 5000;

// RFC 4180's rows: quoted fields, doubled quotes, line breaks inside quotes.
function rows(text) {
  const out = [];
  let row = [];
  let field = "";
  let quoted = false;
  let wasQuoted = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quoted) {
      if (c === '"' && text[i + 1] === '"') {
        field += '"';
        i++;
      } else if (c === '"') quoted = false;
      else field += c;
      continue;
    }
    if (c === '"' && field === "" && !wasQuoted) {
      quoted = wasQuoted = true;
    } else if (c === ",") {
      row.push(field);
      field = "";
      wasQuoted = false;
    } else if (c === "\n") {
      row.push(field);
      out.push(row);
      row = [];
      field = "";
      wasQuoted = false;
    } else if (c !== "\r") field += c;
  }
  if (field !== "" || row.length) {
    row.push(field);
    out.push(row);
  }
  return out;
}

// Milliseconds as Exportify writes them, or m:ss as a hand-made sheet might.
function lengthMs(text) {
  if (/^[+-]?\d+$/.test(text)) {
    const ms = Number(text);
    return ms > 0 ? ms : null;
  }
  const parts = /^([+-]?\d+):([+-]?\d+)$/.exec(text);
  if (!parts) return null;
  const [m, s] = [Number(parts[1]), Number(parts[2])];
  return s < 60 && (m !== 0 || s !== 0) ? (m * 60 + s) * 1000 : null;
}

// The playlist's tracks as { title, artists, album, durationMs, isrc }, or { error } saying
// why the file isn't one, in the words the server would use.
export function readPlaylist(text) {
  const all = rows(text.replace(/^﻿/, "")).filter((row) => row.some((cell) => cell.trim()));
  if (!all.length) return { error: "The file is empty." };
  const columns = all[0].map((c) => c.trim().toLowerCase());
  const find = (names) => {
    for (const name of names) {
      const at = columns.indexOf(name);
      if (at >= 0) return at;
    }
    return null;
  };
  const title = find(TITLE);
  if (title === null) return { error: "The file has no column of track names (like Exportify's \"Track Name\")." };
  const [artist, album, length, isrc] = [find(ARTIST), find(ALBUM), find(LENGTH), find(ISRC)];
  const tracks = [];
  for (const row of all.slice(1)) {
    const cell = (column) => (column === null ? null : row[column]?.trim() || null);
    const name = cell(title);
    if (!name) continue;
    if (tracks.length === MAX_TRACKS) return { error: `The playlist has more than ${MAX_TRACKS} tracks.` };
    tracks.push({
      title: name,
      // Exportify joins several artists with commas.
      artists: (cell(artist) ?? "").split(/[,;]/).map((s) => s.trim()).filter(Boolean),
      album: cell(album),
      durationMs: cell(length) === null ? null : lengthMs(cell(length)),
      isrc: cell(isrc)?.toUpperCase() ?? null,
    });
  }
  if (!tracks.length) return { error: "The file lists no tracks." };
  return { tracks };
}

// A track as the match route takes it: only what the file gave.
const asRow = (t) => Object.fromEntries(Object.entries(t).filter(([, v]) => v !== null && !(Array.isArray(v) && !v.length)));

const byLine = (artists) => (artists?.length ? ` by ${artists.join(", ")}` : "");

// A list in a disclosure, its rows made when it's first opened: a playlist may have thousands.
function listed(summary, items, row) {
  const box = h("details", {}, h("summary", {}, summary));
  box.addEventListener("toggle", () => {
    if (box.open && !box.querySelector("ul")) box.append(h("ul", { class: "plain import-preview-list" }, items.map(row)));
  });
  return box;
}

// The preview's box: empty until a file is read, then what the library has of it. `show`
// matches `tracks` and fills it; a newer call stops an older one's answer from landing.
export function previewBox(ctx) {
  const box = h("div", { class: "import-preview", "aria-live": "polite" });
  let asking = null;
  return {
    box,
    clear() {},
    reset() {
      asking?.abort();
      box.replaceChildren();
    },
    async show(tracks) {
      asking?.abort();
      const mine = (asking = new AbortController());
      box.replaceChildren(h("p", { class: "status" }, `Looking for ${plural(tracks.length, "track", "tracks")} in the library…`));
      let found;
      try {
        found = await api("/aoide/match", { method: "POST", body: tracks.map(asRow), signal: mine.signal });
      } catch (err) {
        if (err.name === "AbortError") return;
        if (err instanceof SignedOut) return ctx.signedOut();
        box.replaceChildren(h("p", { class: "note" }, `Couldn't look in the library first (${err.message}). Importing still works.`));
        return;
      }
      if (mine !== asking || !ctx.current()) return;
      const here = tracks.map((t, i) => [t, found.results[i]]).filter(([, m]) => m);
      const missing = tracks.filter((_, i) => !found.results[i]);
      const n = (count) => number.format(count);
      let words = `${n(here.length)} of the ${plural(tracks.length, "track", "tracks")} ${here.length === 1 ? "is" : "are"} in the library already.`;
      if (!missing.length) words = tracks.length === 1 ? "The track is in the library already." : `All ${n(tracks.length)} tracks are in the library already.`;
      else if (!here.length) words = tracks.length === 1 ? "The track isn't in the library yet." : `None of the ${n(tracks.length)} tracks are in the library yet.`;
      const lists = [];
      if (here.length) {
        lists.push(
          listed(`In the library (${here.length})`, here, ([t, m]) =>
            h("li", {}, h("strong", {}, t.title), byLine(t.artists), m.title !== t.title || byLine(m.artists) !== byLine(t.artists) ? h("span", { class: "note" }, `: as ${m.title ?? "a song"}${byLine(m.artists)}`) : null),
          ),
        );
      }
      if (missing.length) lists.push(listed(`Not in the library (${missing.length})`, missing, (t) => h("li", {}, h("strong", {}, t.title), byLine(t.artists))));
      box.replaceChildren(h("h3", {}, "What the library has"), h("p", {}, words), ...lists);
    },
  };
}
