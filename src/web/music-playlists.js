// Playlists: Aoide's playlists as the devices show them (`GET /playlists`), the caller's own and
// those shared with them. Folders first, then the playlists in none; a folder opens on its own
// (`?folder=` its name) with the folders and playlists in it, a way back up, and Rename and
// Delete. Folders are each person's own, made here or on the devices; a playlist is filed from
// its page. New playlist makes an empty one of the caller's and opens it.
//
// Playlists live in Aoide's sync log, which is closed to someone who may see only some music
// libraries: that refusal (403) is the page's one sentence.

import { SignedOut, api, h, plural, show } from "/web/lib.js";
import { cover } from "/web/music-parts.js";
import { PLAYLISTS_CLOSED, playlistTrouble, playlistsChanged } from "/web/music-playlist-pick.js";
import { clip, emptyState, hold, placeholders, release } from "/web/page-parts.js";
import { seesAllMusic } from "/web/music-access.js";
import { folderHref, folderTools, newFolder } from "/web/music-playlist-folders.js";

export const playlistHref = (id) => `#/music/playlists/${encodeURIComponent(id)}`;

// What a playlist is, under its name: smart, shared, or how many songs.
export function playlistKind(p) {
  if (p.smart) return "Smart playlist";
  if (!p.mine) return `Shared by ${p.owner}`;
  return plural(p.tracks, "song", "songs");
}

function playlistCard(p) {
  return h(
    "li",
    {},
    h(
      "a",
      { class: "card", href: playlistHref(p.id) },
      cover(p.image_url),
      h("div", { class: "title", title: p.name }, p.name),
      h("div", { class: "meta clip", title: playlistKind(p) }, playlistKind(p)),
    ),
  );
}

// A folder, with how many playlists and folders are directly in it.
function folderItem(f, folders) {
  const nested = folders.filter((g) => g.parent_id === f.id).length;
  const count = [plural(f.playlists, "playlist", "playlists"), nested ? plural(nested, "folder", "folders") : null].filter(Boolean).join(", ");
  return h("li", {}, h("a", { href: folderHref(f.name) }, h("span", { class: "name" }, clip(f.name)), h("span", { class: "count" }, count)));
}

// New playlist: a name, then its page.
function newPlaylist() {
  const input = h("input", { id: "new-playlist", maxlength: "200", autocomplete: "off" });
  const button = h("button", { type: "submit" }, "Make playlist");
  const said = h("p", { class: "note", role: "status" });
  const form = h("form", { class: "picker-new new-playlist" }, h("div", {}, h("label", { for: "new-playlist" }, "New playlist"), input), button);
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const name = input.value.trim();
    if (!name) {
      said.textContent = "Give the new playlist a name.";
      input.focus();
      return;
    }
    if (!hold(button)) return;
    try {
      const made = await api("/playlists", { method: "POST", body: { name } });
      playlistsChanged();
      location.hash = playlistHref(made.id);
    } catch (err) {
      if (err instanceof SignedOut) throw err;
      said.textContent = `Couldn't make it: ${playlistTrouble(err)}`;
      release(button);
    }
  });
  return [form, said];
}

export async function playlistsPage(params, ctx) {
  const folder = params.get("folder");
  const title = h("h1", {}, folder ?? "Playlists");
  if (!seesAllMusic()) return show("Playlists", h("h1", {}, "Playlists"), emptyState(PLAYLISTS_CLOSED));
  show(folder ?? "Playlists", title, placeholders("playlists", { kind: "cards", count: 6 }));
  let all;
  let folders;
  try {
    [all, folders] = await Promise.all([api("/playlists"), api("/playlists/folders")]);
  } catch (err) {
    if (err.status !== 403) throw err;
    if (ctx.current()) show("Playlists", title, emptyState(PLAYLISTS_CLOSED));
    return;
  }
  if (!ctx.current()) return;
  const byId = new Map(folders.map((f) => [f.id, f]));
  // A folder opens by its name, as the playlists name theirs.
  const record = folder === null ? null : folders.find((f) => f.name === folder);
  const up = record ? byId.get(record.parent_id) : null;
  const subfolders = folders
    .filter((f) => (record ? f.parent_id === record.id : folder === null && !byId.has(f.parent_id)))
    .sort((a, b) => a.name.localeCompare(b.name));
  const folderList = h(
    "section",
    { class: "listen-row", "aria-labelledby": "playlist-folders" },
    h("h2", { id: "playlist-folders" }, folder === null ? "Folders" : "Folders in it"),
    subfolders.length ? h("ul", { class: "artist-list" }, subfolders.map((f) => folderItem(f, folders))) : null,
    folder === null || record ? newFolder(record?.id ?? null) : null,
  );
  if (folder !== null) {
    const inside = all.filter((p) => p.folder === folder);
    show(
      folder,
      h("p", {}, h("a", { href: up ? folderHref(up.name) : "#/music/playlists" }, up ? up.name : "All playlists")),
      title,
      record ? folderTools(record, up) : null,
      inside.length || subfolders.length
        ? null
        : emptyState(record ? "Nothing is in this folder now. Move a playlist here from its page." : "There's no such folder now.", { href: "#/music/playlists", label: "See every playlist" }),
      record ? folderList : null,
      inside.length
        ? h("section", { class: "listen-row", "aria-labelledby": "playlist-loose" }, h("h2", { id: "playlist-loose" }, "Playlists"), h("ul", { class: "grid" }, inside.map(playlistCard)))
        : null,
    );
    return;
  }
  // Playlists filed in no folder of the caller's, shared ones included.
  const loose = all.filter((p) => !p.folder);
  show(
    "Playlists",
    title,
    newPlaylist(),
    all.length || folders.length ? null : emptyState("No playlists yet. Make one here, or on any of your Aoide devices."),
    folderList,
    loose.length
      ? h(
          "section",
          { class: "listen-row", "aria-labelledby": "playlist-loose" },
          h("h2", { id: "playlist-loose" }, "Not in a folder"),
          h("ul", { class: "grid" }, loose.map(playlistCard)),
        )
      : null,
  );
}
