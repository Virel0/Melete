// Playlist folders, each user's own (`/playlists/folders`): New folder on the playlists page
// and in a folder, Rename and Delete on a folder's page (deleting one deletes no playlist:
// what's in it moves up), and Folder on one's own playlist, to file it in one or take it out.
// Each is written to the sync log as a device writes it, so the Aoide devices follow.

import { confirmInline } from "/web/forms.js";
import { SignedOut, api, h } from "/web/lib.js";
import { playlistTrouble, playlistsChanged } from "/web/music-playlist-pick.js";
import { hold, release } from "/web/page-parts.js";

export const folderHref = (name) => `#/music/playlists?${new URLSearchParams({ folder: name })}`;

let made = 0;

// A folder's name with the folders it's in, for a list of them all: "Moods / Calm".
export function folderPath(folder, byId) {
  const names = [folder.name];
  const seen = new Set([folder.id]);
  for (let up = byId.get(folder.parent_id); up && !seen.has(up.id); up = byId.get(up.parent_id)) {
    names.unshift(up.name);
    seen.add(up.id);
  }
  return names.join(" / ");
}

// A one-field form: a label, a name, a button; `send(name)` returns what to say, or throws.
function nameForm({ label, value = "", button, send, cancel }) {
  const id = `folder-name-${++made}`;
  const input = h("input", { id, maxlength: "200", autocomplete: "off", value });
  const go = h("button", { type: "submit" }, button);
  const said = h("p", { class: "note", role: "status" });
  const form = h(
    "form",
    { class: "picker-new folder-form" },
    h("div", {}, h("label", { for: id }, label), input),
    go,
    cancel ? h("button", { type: "button", class: "quiet", onclick: cancel }, "Cancel") : null,
  );
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const name = input.value.trim();
    if (!name) {
      said.textContent = "Give the folder a name.";
      input.focus();
      return;
    }
    if (!hold(go)) return;
    try {
      said.textContent = (await send(name)) ?? "";
    } catch (err) {
      if (err instanceof SignedOut) throw err;
      said.textContent = `Couldn't save it: ${playlistTrouble(err)}`;
    } finally {
      release(go);
    }
  });
  return { el: h("div", {}, form, said), input };
}

// New folder, at the top or in `parentId`; opens the folder made.
export function newFolder(parentId = null) {
  return nameForm({
    label: parentId ? "New folder in it" : "New folder",
    button: "Make folder",
    send: async (name) => {
      const folder = await api("/playlists/folders", { method: "POST", body: { name, parent_id: parentId } });
      playlistsChanged();
      location.hash = folderHref(folder.name);
      return null;
    },
  }).el;
}

// Rename and Delete for a folder's page. `up` is where its contents go when it's deleted: its
// parent folder, or none for the top.
export function folderTools(folder, up) {
  const box = h("div", { class: "play-actions folder-tools" });
  const rename = h("button", { type: "button", class: "quiet", "aria-expanded": "false" }, "Rename folder");
  const remove = h("button", { type: "button", class: "quiet" }, "Delete folder");
  const area = h("div", {});
  rename.addEventListener("click", () => {
    if (rename.getAttribute("aria-expanded") === "true") {
      rename.setAttribute("aria-expanded", "false");
      area.replaceChildren();
      return;
    }
    rename.setAttribute("aria-expanded", "true");
    const form = nameForm({
      label: "Name",
      value: folder.name,
      button: "Save",
      cancel: () => {
        rename.setAttribute("aria-expanded", "false");
        area.replaceChildren();
        rename.focus();
      },
      send: async (name) => {
        if (name === folder.name) return "That's its name already.";
        await api(`/playlists/folders/${encodeURIComponent(folder.id)}`, { method: "PUT", body: { name } });
        playlistsChanged();
        location.hash = folderHref(name);
        return null;
      },
    });
    area.replaceChildren(form.el);
    form.input.focus();
  });
  remove.addEventListener("click", () =>
    confirmInline(box, {
      question: `Delete the folder ${folder.name}? No playlist is deleted: what's in it moves to ${up ? up.name : "the top"}, on all your devices.`,
      confirm: "Delete folder",
      keep: "Keep it",
      run: async () => {
        await api(`/playlists/folders/${encodeURIComponent(folder.id)}`, { method: "DELETE" });
        playlistsChanged();
        location.hash = up ? folderHref(up.name) : "#/music/playlists";
      },
    }),
  );
  box.append(rename, remove);
  return h("div", {}, box, area);
}

// Folder, on one's own playlist: which of one's folders it's filed in, or none.
export function folderPicker(p, page) {
  const id = `playlist-folder-${++made}`;
  const said = h("p", { class: "note", role: "status" });
  const select = h("select", { id }, h("option", { value: "" }, "Loading your folders…"));
  const save = h("button", { type: "submit" }, "Move");
  const form = h(
    "form",
    { class: "playlist-edit" },
    h("div", { class: "field" }, h("label", { for: id }, "Folder"), select),
    h("div", { class: "form-actions" }, save, h("button", { type: "button", class: "quiet", onclick: () => page.draw(p, null, "", "folder") }, "Cancel")),
    said,
  );
  api("/playlists/folders")
    .then((folders) => {
      const byId = new Map(folders.map((f) => [f.id, f]));
      const options = folders.map((f) => ({ id: f.id, path: folderPath(f, byId) })).sort((a, b) => a.path.localeCompare(b.path));
      select.replaceChildren(
        h("option", { value: "" }, "Not in a folder"),
        ...options.map((f) => h("option", { value: f.id, selected: f.id === p.folder_id }, f.path)),
      );
      if (!folders.length) said.textContent = "You have no folders yet. Make one on the playlists page.";
    })
    .catch((err) => {
      if (err instanceof SignedOut) throw err;
      said.textContent = `Couldn't load your folders: ${playlistTrouble(err)}`;
    });
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    if (!hold(save)) return;
    try {
      const now = await api(`/playlists/${encodeURIComponent(p.id)}`, { method: "PUT", body: { folder_id: select.value || null } });
      playlistsChanged();
      page.draw(now, null, now.folder ? `Moved to ${now.folder}.` : "Taken out of its folder.", "folder");
    } catch (err) {
      if (err instanceof SignedOut) throw err;
      said.textContent = `Couldn't move it: ${playlistTrouble(err)}`;
      release(save);
    }
  });
  return form;
}
