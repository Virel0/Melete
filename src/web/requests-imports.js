// Playlist imports: a playlist's tracks asked for as songs, and how far each import has got.
// Before importing, what the library has of the file is shown; "Also make a playlist" makes
// an Aoide playlist of it too, filled as the import runs.

import { confirmInline, field, form, group, when } from "/web/forms.js";
import { SignedOut, announce, api, h, plural } from "/web/lib.js";
import { seesAllMusic } from "/web/music-access.js";
import { playlistHref } from "/web/music-playlists.js";
import { previewBox, readPlaylist } from "/web/requests-import-preview.js";

// ---------- Playlist imports ----------

const IMPORT_STATES = { queued: "Waiting its turn", running: "Matching tracks…", done: "Done", failed: "Failed" };
// How often an import still being worked through is looked at again.
const IMPORT_POLL_MS = 3000;

// One import: how far it's got, what became of its tracks, and for an admin deciding, a
// button that approves all of it still waiting.
export function importRow(ctx, i, { approving = false } = {}) {
  const counts = [
    `${plural(i.tracks, "track", "tracks")}`,
    i.in_library ? `${i.in_library} in the library already` : null,
    i.asked ? `${i.asked} asked for` : null,
    i.already ? `${i.already} asked for already` : null,
    i.unmatched ? `${i.unmatched} not found` : null,
    i.waiting ? `${i.waiting} waiting for an admin` : null,
  ].filter(Boolean);
  const actions = h("div", { class: "row-actions" });
  if (approving) {
    const said = h("span", { class: "note", role: "status" });
    const all = h("button", { type: "button" }, `Approve all ${i.waiting}`);
    all.addEventListener("click", async () => {
      all.disabled = true;
      said.textContent = "Approving…";
      try {
        const done = await api(`/requests/imports/${i.id}/approve`, { method: "POST" });
        const message = done.refused.length
          ? `Approved ${plural(done.approved.length, "track", "tracks")} of ${i.name}; ${done.refused.length} couldn't be: ${done.refused[0].why}`
          : `Approved ${plural(done.approved.length, "track", "tracks")} of ${i.name}; they're fetched one after another.`;
        ctx.reload(message);
      } catch (err) {
        if (err instanceof SignedOut) return ctx.signedOut();
        said.textContent = err.message;
        all.disabled = false;
      }
    });
    actions.append(all, said);
  } else {
    const remove = h("button", { type: "button", class: "quiet" }, "Remove");
    remove.addEventListener("click", () =>
      confirmInline(actions, {
        question: `Remove ${i.name}? Its track list goes${
          i.waiting ? `, and its ${plural(i.waiting, "song", "songs")} still waiting for an admin ${i.waiting === 1 ? "is" : "are"} taken back` : ""
        }. Songs already approved stay.`,
        confirm: `Remove ${i.name}`,
        keep: "Keep it",
        run: async () => {
          const done = await api(`/requests/imports/${i.id}`, { method: "DELETE" });
          ctx.reload(
            done.taken_back
              ? `Removed ${i.name} and took back ${plural(done.taken_back, "waiting song", "waiting songs")}.`
              : `Removed ${i.name}.`,
          );
        },
      }),
    );
    // The playlist it made, filled as the import runs.
    if (i.playlist_id) actions.append(h("a", { href: playlistHref(i.playlist_id) }, "Open the playlist"));
    actions.append(remove);
  }
  // The tracks that weren't found, fetched when asked to be shown.
  const missing = h("details", {}, h("summary", {}, `Tracks not found (${i.unmatched})`));
  missing.addEventListener("toggle", async () => {
    if (!missing.open || missing.dataset.loaded) return;
    missing.dataset.loaded = "1";
    const list = h("ul", { class: "plain" }, h("li", { class: "status" }, "Loading…"));
    missing.append(list);
    try {
      const full = await api(`/requests/imports/${i.id}`);
      list.replaceChildren(
        ...full.items
          .filter((t) => t.state === "unmatched")
          .map((t) => h("li", {}, h("strong", {}, t.title), t.artist ? ` by ${t.artist}` : "", h("span", { class: "note" }, `: ${t.why}`))),
      );
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      list.replaceChildren(h("li", { class: "error" }, err.message));
      delete missing.dataset.loaded;
    }
  });
  return h(
    "li",
    {},
    h(
      "div",
      {},
      h("strong", {}, i.name),
      " ",
      h("span", { class: `badge${i.state === "failed" ? " state-failed" : ""}` }, IMPORT_STATES[i.state] ?? i.state),
      approving ? h("p", { class: "note" }, `Imported by ${i.username}, ${when(i.created_at)}.`) : h("p", { class: "note" }, `Imported ${when(i.created_at)}.`),
      h("p", {}, `${counts.join(", ")}.`),
      i.state === "failed" && i.message ? h("p", { class: "error" }, i.message) : null,
      i.unmatched ? missing : null,
    ),
    actions,
  );
}

// "Road_Trip.csv", as Exportify names a file, → "Road Trip".
function playlistName(file) {
  return file.name.replace(/\.csv$/i, "").replace(/_/g, " ").trim();
}

// The largest file the server takes.
const MAX_IMPORT_BYTES = 8 * 1024 * 1024;

// A chosen file's text, and its tracks or why it isn't a playlist.
const readFile = async (chosen) => {
  const text = await chosen.text();
  return { chosen, text, ...readPlaylist(text) };
};

// "Also make a playlist" (`make_playlist`), with what it does.
function makePlaylistField() {
  const input = h("input", { type: "checkbox", id: "import-make-playlist", "aria-describedby": "import-make-playlist-hint" });
  const box = h(
    "div",
    {},
    h("div", { class: "check" }, input, h("label", { for: "import-make-playlist" }, "Also make a playlist")),
    h(
      "p",
      { class: "note check-hint", id: "import-make-playlist-hint" },
      "A playlist named as the import, here and on your Aoide devices, with every track in the file's order. Songs the library has go in at once and aren't asked for; the rest go in as each arrives.",
    ),
  );
  return { box, input, clear() {} };
}

export async function imports(ctx) {
  const first = await api("/requests/imports");
  if (!ctx.current()) return [];
  const list = h("ul", { class: "rows" });
  const empty = h("p", { class: "status" }, "No playlists imported yet.");
  let timer = null;
  const draw = (items) => {
    list.replaceChildren(...items.map((i) => importRow(ctx, i)));
    empty.hidden = items.length > 0;
    clearTimeout(timer);
    // Followed while any is still being worked through.
    if (items.some((i) => i.state === "queued" || i.state === "running")) {
      timer = setTimeout(async () => {
        try {
          if (ctx.current()) draw(await api("/requests/imports"));
        } catch (err) {
          if (err instanceof SignedOut) ctx.signedOut();
        }
      }, IMPORT_POLL_MS);
    }
  };
  draw(first);
  ctx.onLeave(() => clearTimeout(timer));
  return [
    group(
      "Import a playlist",
      h("p", { class: "note" }, "Each track becomes a song request, as if you'd asked for it yourself. Songs the library has already aren't asked for, and tracks you've asked for already are left as they are, so importing a playlist again asks only for what's new."),
      importForm(ctx, draw),
    ),
    group("Your playlist imports", empty, list),
  ];
}

// The file, its name, what the library has of it, and Also make a playlist; `draw` lists the
// imports again once one is made.
function importForm(ctx, draw) {
  const file = field(
    "import-file",
    "Playlist file (CSV)",
    h("input", { type: "file", accept: ".csv,text/csv" }),
    "Export the playlist with Exportify (exportify.net), then choose the file it saved.",
  );
  const name = field("import-name", "Name", h("input", { autocomplete: "off", maxlength: 200 }), "Taken from the file's name unless you change it.");
  // Playlists, and the library-wide match the preview uses, are closed to someone who may see
  // only some music libraries; they import as before.
  const all = seesAllMusic();
  const make = makePlaylistField();
  const preview = previewBox(ctx);
  // The chosen file as read here, the way the server reads it.
  let read = null;
  let named = null;
  const tooBig = () => file.fail("That file is over 8 MB. Split the playlist, or export fewer columns.");
  file.input.addEventListener("change", async () => {
    file.clear();
    preview.reset();
    read = null;
    const chosen = file.input.files[0];
    if (!chosen) return;
    // The name follows the file chosen, unless it was typed.
    if (!name.input.value.trim() || name.input.value === named) name.input.value = named = playlistName(chosen);
    if (chosen.size > MAX_IMPORT_BYTES) return tooBig();
    const now = await readFile(chosen);
    if (file.input.files[0] !== chosen) return;
    read = now;
    if (read.error) file.fail(read.error);
    else if (all) preview.show(read.tracks);
  });
  return form(all ? [file, name, preview, make] : [file, name], "Import playlist", async () => {
    const chosen = file.input.files[0];
    if (!chosen) {
      file.fail("Choose the CSV file of the playlist.");
      return null;
    }
    if (chosen.size > MAX_IMPORT_BYTES) {
      tooBig();
      return null;
    }
    if (read?.chosen !== chosen) read = await readFile(chosen);
    if (read.error) {
      file.fail(read.error);
      return null;
    }
    const called = name.input.value.trim() || playlistName(chosen);
    if (!called) {
      name.fail("Give the playlist a name.");
      return null;
    }
    const making = all && make.input.checked;
    const made = await api("/requests/imports", { method: "POST", body: { name: called, csv: read.text, make_playlist: making } });
    if (!ctx.current()) return null;
    draw(await api("/requests/imports"));
    announce(`Importing ${made.name}`);
    const tracks = plural(made.tracks, "track", "tracks");
    return making
      ? `Importing ${tracks} from ${made.name} into a new playlist of that name. Each the library lacks is matched on MusicBrainz and asked for as a song.`
      : `Importing ${tracks} from ${made.name}. Each is matched on MusicBrainz and asked for as a song.`;
  });
}
