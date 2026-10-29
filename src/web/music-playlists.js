// Playlists: Aoide's playlists as the devices show them (`GET /playlists`), the caller's own and
// those shared with them. Folders first, then the playlists in none; a folder opens on its own
// with a way back up (`?folder=`). Folders are each person's own and are arranged on the
// devices. New playlist makes an empty one of the caller's and opens it.
//
// Playlists live in Aoide's sync log, which is closed to someone who may see only some music
// libraries: that refusal (403) is the page's one sentence.

import { SignedOut, api, h, plural, show } from "/web/lib.js";
import { cover } from "/web/music-parts.js";
import { PLAYLISTS_CLOSED, playlistTrouble, playlistsChanged } from "/web/music-playlist-pick.js";
import { clip, emptyState, hold, placeholders, release } from "/web/page-parts.js";
import { seesAllMusic } from "/web/music-access.js";

export const playlistHref = (id) => `#/music/playlists/${encodeURIComponent(id)}`;
const folderHref = (name) => `#/music/playlists?${new URLSearchParams({ folder: name })}`;

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

function folderItem(name, count) {
  return h(
    "li",
    {},
    h("a", { href: folderHref(name) }, h("span", { class: "name" }, clip(name)), h("span", { class: "count" }, plural(count, "playlist", "playlists"))),
  );
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
  const back = folder ? h("p", {}, h("a", { href: "#/music/playlists" }, "All playlists")) : null;
  if (!seesAllMusic()) return show("Playlists", h("h1", {}, "Playlists"), emptyState(PLAYLISTS_CLOSED));
  show(folder ?? "Playlists", back, title, placeholders("playlists", { kind: "cards", count: 6 }));
  let all;
  try {
    all = await api("/playlists");
  } catch (err) {
    if (err.status !== 403) throw err;
    if (ctx.current()) show("Playlists", title, emptyState(PLAYLISTS_CLOSED));
    return;
  }
  if (!ctx.current()) return;
  if (folder) {
    const inside = all.filter((p) => p.folder === folder);
    show(
      folder,
      back,
      title,
      inside.length
        ? h("ul", { class: "grid" }, inside.map(playlistCard))
        : emptyState("Nothing is in this folder now.", { href: "#/music/playlists", label: "See every playlist" }),
    );
    return;
  }
  const folders = new Map();
  for (const p of all) if (p.folder) folders.set(p.folder, (folders.get(p.folder) ?? 0) + 1);
  const loose = all.filter((p) => !p.folder);
  show(
    "Playlists",
    title,
    newPlaylist(),
    all.length ? null : emptyState("No playlists yet. Make one here, or on any of your Aoide devices."),
    folders.size
      ? h(
          "section",
          { class: "listen-row", "aria-labelledby": "playlist-folders" },
          h("h2", { id: "playlist-folders" }, "Folders"),
          h("ul", { class: "artist-list" }, [...folders].sort(([a], [b]) => a.localeCompare(b)).map(([name, count]) => folderItem(name, count))),
        )
      : null,
    loose.length
      ? h(
          "section",
          { class: "listen-row", "aria-labelledby": "playlist-loose" },
          folders.size ? h("h2", { id: "playlist-loose" }, "Not in a folder") : h("h2", { id: "playlist-loose", class: "visually-hidden" }, "Your playlists"),
          h("ul", { class: "grid" }, loose.map(playlistCard)),
        )
      : null,
  );
}
