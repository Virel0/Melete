// An album's actions beyond Play and Shuffle, for its page and its cards: Play next and Play
// last put the whole album in Next up (queueButtons), and an album card's More button opens
// the album's actions over the card (albumMenu): Play, Shuffle, Play next, Play last, Start
// radio and Add to playlist, as on the album's page. A card asks for the album's tracks only
// when an action needs them.
//
// A card's actions are a popover: one is open at a time, and Escape, or a press outside, closes
// it and puts focus back on More.

import { SignedOut, api, h } from "/web/lib.js";
import { iconButton } from "/web/music-controls.js";
import { playLast, playNext } from "/web/music-player.js";
import { albumQueue, play, shufflePlay } from "/web/music-play.js";
import { playlistPicker } from "/web/music-playlist-pick.js";
import { albumSeed, radioButton } from "/web/music-radio.js";
import { hold, release } from "/web/page-parts.js";
import { seesAllMusic } from "/web/music-access.js";

let made = 0;

// Play next and Play last for an album, saying what they did in `said`. `queue()` gives the
// album's queue entries (it may ask the server).
export function queueButtons(album, queue, said, extra = "") {
  const button = (words, how, done) => {
    const b = h("button", { type: "button", class: `quiet ${extra}`.trim() }, words, h("span", { class: "visually-hidden" }, `: ${album.title}`));
    b.addEventListener("click", async () => {
      if (!hold(b)) return;
      said.textContent = "";
      try {
        const entries = await queue();
        if (!entries.length) {
          said.textContent = "This album has no songs in the library now.";
          return;
        }
        how(entries);
        said.textContent = done;
      } catch (err) {
        if (err instanceof SignedOut) throw err;
        said.textContent = `Couldn't add it: ${err.message}`;
      } finally {
        release(b);
      }
    });
    return b;
  };
  return [
    button("Play next", playNext, `${album.title} plays next.`),
    button("Play last", playLast, `${album.title} is added to the end of Next up.`),
  ];
}

// The actions inside a card's menu, made when it first opens.
function menuBody(album, close) {
  let asked = null;
  const tracks = () => (asked ??= api(`/music/albums/${album.id}`).catch((err) => {
    asked = null;
    throw err;
  }));
  const queue = async () => albumQueue(await tracks());
  const said = h("p", { class: "note", role: "status" });
  // Play and Shuffle start the album and close the menu; the player shows what's on.
  const start = (words, how) => {
    const b = h("button", { type: "button", class: "quiet small" }, words);
    b.addEventListener("click", async () => {
      if (!hold(b)) return;
      said.textContent = "";
      try {
        const entries = await queue();
        if (!entries.length) {
          said.textContent = "This album has no songs in the library now.";
          return;
        }
        how(entries);
        close();
      } catch (err) {
        if (err instanceof SignedOut) throw err;
        said.textContent = `Couldn't play it: ${err.message}`;
      } finally {
        release(b);
      }
    });
    return b;
  };
  const area = h("div", { class: "pick-area" });
  const pick = h("button", { type: "button", class: "quiet small", "aria-expanded": "false" }, "Add to playlist");
  pick.addEventListener("click", async () => {
    const opening = pick.getAttribute("aria-expanded") !== "true";
    pick.setAttribute("aria-expanded", String(opening));
    if (!opening) return area.replaceChildren();
    try {
      const album = await tracks();
      area.replaceChildren(playlistPicker(album.track_list.map((t) => t.id), album.title));
    } catch (err) {
      if (err instanceof SignedOut) throw err;
      pick.setAttribute("aria-expanded", "false");
      said.textContent = `Couldn't read the album: ${err.message}`;
    }
  });
  const synced = seesAllMusic();
  const [radio, radioSaid] = radioButton(() => albumSeed(album.id), album.title);
  radio?.classList.add("small");
  return [
    h(
      "div",
      { class: "card-menu-buttons" },
      start("Play", (q) => play(q)),
      start("Shuffle", shufflePlay),
      queueButtons(album, queue, said, "small"),
      synced ? [radio, pick] : null,
    ),
    area,
    said,
    radioSaid ?? null,
  ];
}

// Puts the open menu under its More button, kept on the screen. It sits in the top layer, so a
// shelf scrolled sideways doesn't clip it.
function place(panel, more) {
  const at = more.getBoundingClientRect();
  const gutter = 16;
  const width = Math.min(panel.offsetWidth, innerWidth - 2 * gutter);
  const left = Math.max(gutter, Math.min(at.left, innerWidth - width - gutter));
  const below = at.bottom + 4;
  const fits = innerHeight - below - gutter >= panel.offsetHeight;
  const top = fits ? below : Math.max(gutter, at.top - 4 - panel.offsetHeight);
  panel.style.setProperty("left", `${left}px`);
  panel.style.setProperty("top", `${top}px`);
}

// A card's More button and the actions it opens, for the card to place.
export function albumMenu(album) {
  const id = `album-menu-${++made}`;
  const panel = h("div", { class: "card-menu", id, popover: "auto", role: "group", "aria-label": `Actions for ${album.title}` });
  const more = iconButton("more", `More for ${album.title}`, null, "card-more");
  more.setAttribute("popovertarget", id);
  let built = false;
  // The page scrolling or the window resizing takes the button away from the menu: the menu
  // closes.
  const scrolled = (event) => {
    if (!panel.contains(event.target)) panel.hidePopover();
  };
  const resized = () => panel.hidePopover();
  panel.addEventListener("beforetoggle", (event) => {
    if (event.newState !== "open" || built) return;
    panel.append(...menuBody(album, () => panel.hidePopover()).flat());
    built = true;
  });
  panel.addEventListener("toggle", (event) => {
    if (event.newState !== "open") {
      removeEventListener("scroll", scrolled, true);
      removeEventListener("resize", resized);
      return;
    }
    place(panel, more);
    addEventListener("scroll", scrolled, true);
    addEventListener("resize", resized);
    panel.querySelector("button")?.focus();
  });
  return [more, panel];
}
