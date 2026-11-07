// Home's Jump back in: what the listener started lately (`/music/recent`: albums, playlists and
// stations, radio from a song), then the albums Home found in their play history, at most
// JUMP_BACK. Each card plays at once; each has Remove, which keeps it out of the row until it's
// started again. A removed album stays out of what the history brings too.

import { SignedOut, api, h } from "/web/lib.js";
import { clip, hold, release } from "/web/page-parts.js";
import { cover } from "/web/music-parts.js";
import { iconButton } from "/web/music-controls.js";
import { looseQueue, play, playAlbum, remember } from "/web/music-play.js";
import { radioTrouble, startRadio } from "/web/music-radio.js";

const JUMP_BACK = 8;

// An entry as a card's words, picture and what pressing it does.
function described(entry) {
  if (entry.kind === "album") {
    const a = entry.album;
    return { name: a.title, by: a.artist, picture: a.cover, start: () => playAlbum(a.id) };
  }
  if (entry.kind === "playlist") {
    const p = entry.playlist;
    const start = async () => {
      const list = await api(`/playlists/${encodeURIComponent(p.id)}`);
      const tracks = list.items.map((i) => i.track).filter(Boolean);
      play(looseQueue(tracks));
      remember("playlist", p.id);
    };
    return { name: p.name, by: p.mine ? "Playlist" : `Playlist shared by ${p.owner}`, picture: p.image_url, start };
  }
  const t = entry.track;
  return { name: `Radio from ${t.title}`, by: t.artist ?? t.album_artist, picture: null, start: () => startRadio(looseQueue([t])[0]) };
}

// The recorded entries first, then the history's albums not among them and not removed.
export function jumpEntries(recent, history) {
  const removed = new Set(recent.removed_albums);
  const recorded = new Set(recent.items.filter((e) => e.kind === "album").map((e) => e.album.id));
  const fromHistory = history.filter((a) => !removed.has(a.id) && !recorded.has(a.id)).map((album) => ({ kind: "album", id: String(album.id), album }));
  return [...recent.items, ...fromHistory].slice(0, JUMP_BACK);
}

// The row, or null with nothing in it.
export function jumpBackIn(recent, history) {
  const entries = jumpEntries(recent, history);
  if (!entries.length) return null;
  const said = h("p", { class: "note", role: "status" });
  const list = h("ul", { class: "jump" });
  const card = (entry) => {
    const { name, by, picture, start } = described(entry);
    const button = h("button", { type: "button", class: "jump-card", "aria-label": `Play ${name}, ${by}` }, cover(picture), h("span", { class: "words" }, clip(name, "strong"), clip(by)));
    button.addEventListener("click", async () => {
      if (!hold(button)) return;
      button.setAttribute("aria-busy", "true");
      said.textContent = "";
      try {
        await start();
      } catch (err) {
        if (err instanceof SignedOut) throw err;
        said.textContent = entry.kind === "station" ? radioTrouble(err) : `Couldn't play it: ${err.message}`;
      } finally {
        release(button);
        button.removeAttribute("aria-busy");
      }
    });
    const remove = iconButton("close", `Remove ${name} from Jump back in`, async () => {
      if (!hold(remove)) return;
      try {
        await api(`/music/recent/${entry.kind}/${encodeURIComponent(entry.id)}`, { method: "DELETE" });
        const item = remove.closest("li");
        const next = item.nextElementSibling ?? item.previousElementSibling;
        item.remove();
        said.textContent = `${name} is out of Jump back in until you play it again.`;
        next?.querySelector(".jump-card")?.focus();
      } catch (err) {
        if (err instanceof SignedOut) throw err;
        said.textContent = `Couldn't remove it: ${err.message}`;
        release(remove);
      }
    }, "jump-remove");
    return h("li", {}, button, remove);
  };
  list.append(...entries.map(card));
  return [list, said];
}
