// Add to playlist: the caller's own hand-made playlists (the only ones the web edits; one shared
// with them, or a smart one, is changed on their devices), each a button that adds the tracks
// at the end, and New playlist, which makes one and adds them to it. A search field narrows
// the list once there are more than a few. Every change is sent as `melete:playlists`, so what
// lists playlists can follow.
//
// Playlists live in Aoide's sync log, which is closed to someone who may see only some music
// libraries: that refusal (403) is said in a sentence.

import { SignedOut, api, h, plural } from "/web/lib.js";
import { hold, placeholders, release } from "/web/page-parts.js";

const SEARCH_FROM = 8;
let made = 0;

export const PLAYLISTS_CLOSED = "Playlists aren't open to you, as you can see only some music libraries. An admin can change what you see.";

export function playlistsChanged() {
  document.dispatchEvent(new CustomEvent("melete:playlists"));
}

// What went wrong with a playlist, in a sentence.
export function playlistTrouble(err) {
  if (err.status === 403) return err.message.includes("only some music libraries") ? PLAYLISTS_CLOSED : `Only its owner changes it here: ${err.message}`;
  if (err.status === 409) return `It can't be changed here: ${err.message}`;
  return err.message;
}

// The picker for `trackIds`; `what` names them ("Harbour Lights", "this album") in what it says.
export function playlistPicker(trackIds, what) {
  const id = `pick-${++made}`;
  const said = h("p", { class: "note", role: "status" });
  const body = h("div", { class: "picker-body" }, placeholders("your playlists", { kind: "lines", count: 3 }));
  const box = h("div", { class: "playlist-picker", role: "group", "aria-label": `Add ${what} to a playlist` }, body, said);
  const songs = plural(trackIds.length, "song", "songs");

  const add = async (button, playlist) => {
    if (!hold(button)) return;
    said.textContent = "";
    try {
      await api(`/playlists/${encodeURIComponent(playlist.id)}/items`, { method: "POST", body: { track_ids: trackIds } });
      said.textContent = `Added ${what} to ${playlist.name}.`;
      playlistsChanged();
    } catch (err) {
      if (err instanceof SignedOut) throw err;
      said.textContent = `Couldn't add ${songs}: ${playlistTrouble(err)}`;
    } finally {
      release(button);
    }
  };

  const create = (input, button) => async (event) => {
    event.preventDefault();
    const name = input.value.trim();
    if (!name) {
      said.textContent = "Give the new playlist a name.";
      input.focus();
      return;
    }
    if (!hold(button)) return;
    said.textContent = "";
    try {
      const playlist = await api("/playlists", { method: "POST", body: { name } });
      await api(`/playlists/${encodeURIComponent(playlist.id)}/items`, { method: "POST", body: { track_ids: trackIds } });
      said.textContent = `Made ${playlist.name} and added ${what} to it.`;
      input.value = "";
      playlistsChanged();
      draw();
    } catch (err) {
      if (err instanceof SignedOut) throw err;
      said.textContent = `Couldn't make it: ${playlistTrouble(err)}`;
    } finally {
      release(button);
    }
  };

  const newForm = () => {
    const input = h("input", { id: `${id}-name`, maxlength: "200", autocomplete: "off" });
    const button = h("button", { type: "submit", class: "quiet" }, "Make it and add");
    const form = h("form", { class: "picker-new" }, h("div", {}, h("label", { for: `${id}-name` }, "New playlist"), input), button);
    form.addEventListener("submit", create(input, button));
    return form;
  };

  const list = (mine) => {
    const items = mine.map((p) => {
      const button = h("button", { type: "button", class: "quiet small" }, p.name, h("span", { class: "count" }, plural(p.tracks, "song", "songs")));
      button.addEventListener("click", () => add(button, p));
      return h("li", { "data-name": p.name.toLowerCase() }, button);
    });
    const ul = h("ul", { class: "picker-list", "aria-label": "Your playlists" }, items);
    if (mine.length <= SEARCH_FROM) return [ul];
    const search = h("input", { id: `${id}-find`, type: "search", autocomplete: "off" });
    search.addEventListener("input", () => {
      const q = search.value.trim().toLowerCase();
      for (const li of items) li.hidden = Boolean(q) && !li.dataset.name.includes(q);
    });
    return [h("div", { class: "picker-find" }, h("label", { for: `${id}-find` }, "Find a playlist"), search), ul];
  };

  async function draw() {
    try {
      const all = await api("/playlists");
      const mine = all.filter((p) => p.mine && !p.smart);
      body.replaceChildren(...(mine.length ? list(mine) : [h("p", { class: "note" }, "You have no playlists of your own yet.")]), newForm());
    } catch (err) {
      if (err instanceof SignedOut) throw err;
      body.replaceChildren(h("p", {}, playlistTrouble(err)));
    }
  }
  draw();
  return box;
}
