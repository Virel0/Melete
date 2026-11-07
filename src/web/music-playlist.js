// One playlist: its cover, name, owner and notes, Play, Shuffle, Start radio (from its first song,
// as the Aoide apps start it) and Pin, then its songs. The caller's own hand-made playlist, or one
// shared with them to edit, is edited here, through the same sync log the devices use: Rename
// (with its notes), Picture (choose one, or remove it), and each song moved up, down or out; only
// the owner gets Delete (said plainly first), and files it in a folder (Folder) and shares it
// (Share); songs are added from Add to playlist beside any song or on an album. A playlist shared
// with the caller to listen to is read-only here. A smart one's songs are worked out by the server
// from its rules and the listener's own likes and listens (`/playlists/{id}/songs`), as on the
// devices, and play like any list; its owner changes its rules here (Edit rules) or on the
// devices. A song gone from the library stays, greyed, with what it was; one not in it yet shows
// greyed too, saying how its request stands, until the server swaps its track in.

import { SignedOut, api, clock, h, metaRow, plural, runtime, show } from "/web/lib.js";
import { trackActions } from "/web/music-actions.js";
import { iconButton } from "/web/music-controls.js";
import { goneRow } from "/web/music-liked.js";
import { cover, markPlaying, pinControl, songRow } from "/web/music-parts.js";
import { looseQueue, play, remember, shufflePlay } from "/web/music-play.js";
import { radioButton } from "/web/music-radio.js";
import { PLAYLISTS_CLOSED, playlistTrouble, playlistsChanged } from "/web/music-playlist-pick.js";
import { folderPicker } from "/web/music-playlist-folders.js";
import { shareSection } from "/web/music-playlist-share.js";
import { smartEditor } from "/web/music-smart-editor.js";
import { PICTURE_TYPES, readPicture } from "/web/picture-file.js";
import { emptyState, hold, placeholders, release } from "/web/page-parts.js";
import { seesAllMusic } from "/web/music-access.js";

const path = (p) => `/playlists/${encodeURIComponent(p.id)}`;

// Who can change it, when it isn't the caller's own hand-made playlist.
function whoEdits(p) {
  if (p.smart) {
    const own = p.mine ? "change them with Edit rules" : `only ${p.owner} changes them`;
    return `Its songs are worked out from its rules on your Aoide devices and here, from your own likes and listens; ${own}.`;
  }
  if (p.mine) return null;
  return p.can_edit ? `${p.owner} shares it with you to change, here and on your Aoide devices; only they delete it or share it.` : `Only ${p.owner} changes it.`;
}

// Rename: its name and notes, saved together.
function renameForm(p, page) {
  const name = h("input", { id: "playlist-name", value: p.name, maxlength: "200", required: true, autocomplete: "off" });
  const notes = h("textarea", { id: "playlist-notes", maxlength: "2000", rows: "3" }, p.notes ?? "");
  const said = h("p", { class: "note", role: "status" });
  const save = h("button", { type: "submit" }, "Save");
  const form = h(
    "form",
    { class: "playlist-edit" },
    h("div", { class: "field" }, h("label", { for: "playlist-name" }, "Name"), name),
    h("div", { class: "field" }, h("label", { for: "playlist-notes" }, "Notes"), notes),
    h("div", { class: "form-actions" }, save, h("button", { type: "button", class: "quiet", onclick: () => page.draw(p, null, "", "rename") }, "Cancel")),
    said,
  );
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    if (!name.value.trim()) {
      said.textContent = "Give it a name.";
      name.focus();
      return;
    }
    if (!hold(save)) return;
    try {
      const saved = await api(path(p), { method: "PUT", body: { name: name.value.trim(), notes: notes.value.trim() || null } });
      playlistsChanged();
      page.draw(saved, null, "Saved.", "rename");
    } catch (err) {
      if (err instanceof SignedOut) throw err;
      said.textContent = `Couldn't save it: ${playlistTrouble(err)}`;
      release(save);
    }
  });
  return form;
}

// Picture: choosing a file sets it at once; Remove picture when it has one.
function pictureForm(p, page) {
  const said = h("p", { class: "note", role: "status" });
  const input = h("input", { type: "file", id: "playlist-picture", accept: PICTURE_TYPES.join(","), "aria-describedby": "playlist-picture-hint" });
  const send = async (busy, request, done) => {
    if (!hold(busy)) return;
    said.textContent = request.body ? "Saving the picture…" : "";
    try {
      const saved = await api(`${path(p)}/image`, request);
      playlistsChanged();
      page.draw(saved, null, done, "picture");
    } catch (err) {
      if (err instanceof SignedOut) throw err;
      said.textContent = `Couldn't change the picture: ${playlistTrouble(err)}`;
      release(busy);
    }
  };
  input.addEventListener("change", async () => {
    const chosen = input.files?.[0];
    if (!chosen) return;
    const read = await readPicture(chosen);
    if (read.error) {
      said.textContent = read.error;
      return;
    }
    send(input, { method: "PUT", body: read.blob }, "Picture saved.");
  });
  const remove = p.image_url ? h("button", { type: "button", class: "quiet" }, "Remove picture") : null;
  remove?.addEventListener("click", () => send(remove, { method: "DELETE" }, "Picture removed."));
  return h(
    "div",
    { class: "playlist-edit" },
    h(
      "div",
      { class: "field" },
      h("label", { for: "playlist-picture" }, p.image_url ? "Choose another picture" : "Choose a picture"),
      h("p", { class: "note", id: "playlist-picture-hint" }, "JPEG, PNG or WebP, up to 5 MB. Your Aoide devices show it after they next sync."),
      input,
    ),
    h("div", { class: "form-actions" }, remove, h("button", { type: "button", class: "quiet", onclick: () => page.draw(p, null, "", "picture") }, "Done")),
    said,
  );
}

// Delete, said plainly first, with keeping it the easy choice.
function deleteCheck(p, page) {
  const said = h("p", { class: "note", role: "status" });
  const keep = h("button", { type: "button", class: "quiet", onclick: () => page.draw(p, null, "", "delete") }, "Keep it");
  const gone = h("button", { type: "button", class: "danger" }, "Delete playlist");
  gone.addEventListener("click", async () => {
    if (!hold(gone)) return;
    try {
      await api(path(p), { method: "DELETE" });
      playlistsChanged();
      location.hash = "#/music/playlists";
    } catch (err) {
      if (err instanceof SignedOut) throw err;
      said.textContent = `Couldn't delete it: ${playlistTrouble(err)}`;
      release(gone);
    }
  });
  setTimeout(() => keep.focus());
  return h(
    "div",
    { class: "playlist-delete", role: "group", "aria-label": `Delete ${p.name}` },
    h("p", {}, `Delete ${p.name}? It goes from all your devices, and from anyone you share it with. Its songs stay in the library.`),
    h("div", { class: "form-actions" }, keep, gone),
    said,
  );
}

// Move up, Move down and Remove for item `i`, for the playlist's owner.
function itemTools(p, i, name, page) {
  const item = p.items[i];
  if (!item.id) return null;
  const change = (what, request) => async () => {
    try {
      const now = await api(`${path(p)}/items/${encodeURIComponent(item.id)}`, request);
      playlistsChanged();
      page.draw(now, null, "", { item: item.id, what, at: i });
    } catch (err) {
      if (err instanceof SignedOut) throw err;
      page.say(`Couldn't change it: ${playlistTrouble(err)}`);
    }
  };
  const last = p.items.length - 1;
  return h(
    "div",
    { class: "row-tools item-tools" },
    iconButton("up", `Move ${name} up`, change("up", { method: "PUT", body: { index: i - 1 } }), i === 0 ? "unavailable" : ""),
    iconButton("down", `Move ${name} down`, change("down", { method: "PUT", body: { index: i + 1 } }), i === last ? "unavailable" : ""),
    iconButton("close", `Remove ${name} from ${p.name}`, change("remove", { method: "DELETE" })),
  );
}

// How a pending song's request stands, as its row says it.
const WAITING = {
  requested: "Requested",
  approved: "On its way",
  declined: "Request declined",
  unmatched: "Not found on MusicBrainz",
};

// A song not in the library yet: what it is, and how its request stands.
function pendingRow(song) {
  return h(
    "li",
    { class: "gone pending" },
    h("span", { class: "name" }, h("span", {}, song.title), h("span", { class: "by" }, [song.artist, song.album].filter(Boolean).join(", "))),
    h("span", { class: "missing" }, WAITING[song.state] ?? "Not in the library"),
  );
}

function itemList(p, page, editable) {
  const here = p.items.filter((i) => i.track).map((i) => i.track);
  const queue = looseQueue(here);
  return h(
    "ul",
    { class: `tracks songs playlist-items${editable ? " editable" : ""}` },
    p.items.map((item, i) => {
      const name = item.track?.title ?? item.pending?.title ?? (item.content_key ?? "").split("|")[2] ?? "this song";
      const tools = editable ? itemTools(p, i, name, page) : null;
      const row = item.track
        ? songRow(item.track, () => play(queue, here.indexOf(item.track)), trackActions(item.track, queue[here.indexOf(item.track)]))
        : item.pending
          ? pendingRow(item.pending)
          : goneRow(item.content_key);
      if (tools) row.append(tools);
      row.dataset.item = item.id ?? "";
      return row;
    }),
  );
}

// The songs it plays: a smart playlist's as worked out, else its items' tracks.
const songsOf = (p, page) => (p.smart ? (page.songs ?? []) : p.items.filter((i) => i.track).map((i) => i.track));

function head(p, page, editable, open) {
  const here = songsOf(p, page);
  const queue = looseQueue(here);
  const seconds = here.reduce((sum, t) => sum + (t.duration_seconds ?? 0), 0);
  const toggle = (what, label) =>
    h(
      "button",
      { type: "button", class: "quiet", "data-opens": what, "aria-expanded": String(open === what), onclick: () => page.draw(p, open === what ? null : what, "", open === what ? what : null) },
      label,
    );
  return h(
    "div",
    { class: "album-head playlist-head" },
    h("div", { class: "album-cover" }, p.image_url ? h("img", { src: p.image_url, alt: "" }) : null),
    h(
      "div",
      { class: "facts" },
      h("p", { class: "kind" }, p.smart ? "Smart playlist" : "Playlist"),
      h("h1", {}, p.name),
      p.mine ? null : h("p", { class: "by-line" }, `Shared by ${p.owner}`),
      p.notes ? h("p", { class: "playlist-notes" }, p.notes) : null,
      p.smart && !page.songs ? null : metaRow([plural(p.smart ? here.length : p.items.length, "song", "songs"), runtime(Math.round(seconds / 60)) ?? (seconds ? clock(seconds) : null)]),
      h(
        "div",
        { class: "play-actions" },
        here.length ? h("button", { type: "button", onclick: () => (play(queue), remember("playlist", p.id)) }, "Play") : null,
        here.length ? h("button", { type: "button", class: "quiet", onclick: () => (shufflePlay(queue), remember("playlist", p.id)) }, "Shuffle") : null,
        here.length ? radioButton(async () => queue[0], p.name) : null,
        editable ? [toggle("rename", "Rename"), toggle("picture", "Picture"), p.mine ? toggle("delete", "Delete") : null] : null,
        p.mine ? [toggle("folder", "Folder"), toggle("share", "Share")] : null,
        p.mine && p.smart ? toggle("rules", "Edit rules") : null,
        pinControl("playlist", p.id, p.name),
      ),
      open === "rename" ? renameForm(p, page) : null,
      open === "picture" ? pictureForm(p, page) : null,
      open === "delete" ? deleteCheck(p, page) : null,
      open === "folder" ? folderPicker(p, page) : null,
      open === "share" ? shareSection(p) : null,
      open === "rules" ? rulesEditor(p, page) : null,
      whoEdits(p) ? h("p", { class: "note" }, whoEdits(p)) : null,
    ),
  );
}

export async function playlistPage(id, ctx) {
  if (!seesAllMusic()) {
    return show("Playlist", h("h1", {}, "Playlist"), emptyState(PLAYLISTS_CLOSED, { href: "#/music", label: "Go to Listen" }));
  }
  show("Playlist", placeholders("the playlist", { kind: "rows" }));
  let p;
  try {
    p = await api(`/playlists/${encodeURIComponent(id)}`);
  } catch (err) {
    if (err.status !== 403 && err.status !== 404) throw err;
    const why = err.status === 403 ? PLAYLISTS_CLOSED : "There's no such playlist: it may have been deleted, or it isn't shared with you.";
    if (ctx.current()) show("Playlist", h("h1", {}, "Playlist"), emptyState(why, { href: "#/music/playlists", label: "See your playlists" }));
    return;
  }
  if (!ctx.current()) return;
  const said = h("p", { class: "note", role: "status" });
  const worked = p.smart ? await smartSongs(p) : null;
  if (!ctx.current()) return;
  const page = {
    songs: worked?.tracks,
    songsTrouble: worked?.trouble,
    rules: worked?.rules,
    say: (text) => (said.textContent = text),
    // Draws the playlist as it is now: `open` the form shown, `words` said, and `focus` where
    // the keyboard was (a song's { item, what, at }, or "rename", "picture" or "delete" for the
    // button that opened that form), so an edit doesn't lose its place.
    draw(now, open = null, words = "", focus = null) {
      const editable = now.mine && !now.smart;
      // Shared with them to edit, they change it as the owner does, but for Delete.
      const changing = editable || (now.can_edit && !now.smart);
      const list = now.smart ? smartList(page) : now.items.length ? itemList(now, page, changing) : emptyState(changing ? "Nothing in it yet. Add songs with Add to playlist, beside any song or on an album." : "Nothing in it yet.", { href: "#/music/songs", label: "Go to Songs" });
      show(now.name, h("p", {}, h("a", { href: "#/music/playlists" }, "All playlists")), head(now, page, changing, open), said, list);
      said.textContent = words;
      if (list?.matches("ul")) markPlaying(list, ctx);
      if (typeof focus === "string") document.querySelector(`[data-opens="${focus}"]`)?.focus();
      else if (focus) refocus(focus);
      else if (open) document.querySelector({ rename: "#playlist-name", picture: "#playlist-picture", folder: ".playlist-edit select", rules: ".smart-editor select", delete: ".playlist-delete button" }[open] ?? null)?.focus();
    },
  };
  page.draw(p);
}

// Edit rules, on one's own smart playlist: the editor with its rules, and once they're saved,
// its songs worked out again.
function rulesEditor(p, page) {
  return smartEditor({
    rules: page.rules,
    save: async (rules) => {
      const saved = await api(`${path(p)}/rules`, { method: "PUT", body: { rules } });
      const worked = await smartSongs(saved);
      Object.assign(page, { songs: worked.tracks, songsTrouble: worked.trouble, rules: worked.rules });
      playlistsChanged();
      page.draw(saved, null, "Rules saved.", "rules");
      return null;
    },
    cancel: () => page.draw(p, null, "", "rules"),
  });
}

// A smart playlist's songs for the listener now, on their own calendar: { tracks }, or
// { trouble } saying why its rules can't be worked out.
async function smartSongs(p) {
  try {
    const worked = await api(`${path(p)}/songs?offset_minutes=${-new Date().getTimezoneOffset()}`);
    return { tracks: worked.tracks, rules: worked.rules };
  } catch (err) {
    if (err instanceof SignedOut) throw err;
    return { trouble: err.message };
  }
}

// A smart playlist's songs, each played with the rest after it; or why there are none.
function smartList(page) {
  // The server says why in a sentence of its own ("its rules can't be worked out: …").
  if (page.songsTrouble) return h("p", { class: "error" }, `${page.songsTrouble[0].toUpperCase()}${page.songsTrouble.slice(1)}.`);
  if (!page.songs.length) return emptyState("No songs fit its rules now. They change as you listen, like and add music.");
  const queue = looseQueue(page.songs);
  return h(
    "ul",
    { class: "tracks songs playlist-items" },
    page.songs.map((t, i) => songRow(t, () => play(queue, i), trackActions(t, queue[i]))),
  );
}

// After a move or a removal: the same button on the same song, or the song now in its place.
function refocus({ item, what, at }) {
  const rows = [...document.querySelectorAll(".playlist-items > li")];
  const row = rows.find((r) => r.dataset.item === item) ?? rows[Math.min(at, rows.length - 1)];
  const pick = { up: 'button[aria-label$=" up"]', down: 'button[aria-label$=" down"]', remove: 'button[aria-label^="Remove"]' }[what];
  const target = row?.querySelector(pick);
  (target && !target.classList.contains("unavailable") ? target : row?.querySelector("button"))?.focus();
}
