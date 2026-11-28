// The bar at the foot of a phone's screen: the side's main sections within thumb reach, each an
// icon over its name, the current one in the accent. The header keeps the side switch, Search
// and the account; the bar shows only below 640 px (shell.css), and not on the player.
//
// drawTabBar(entries, route, { hidden }) fills it from the side's entries (sides.js): four main
// ones, and More when there are others, which opens them as a sheet from the foot of the screen.
// Manage's services carry their health: a dot on the icon, the words read out with the name.

import { h } from "/web/lib.js";
import { icon } from "/web/music-controls.js";

const bar = document.getElementById("tab-bar");
const sheet = document.getElementById("tab-more");

// Line icons on a 24 px grid, drawn as the player's are (music-controls.js draws `more`).
const PATHS = {
  home: "M4 11 12 4.5l8 6.5v9h-5.5v-5.5h-5V20H4z",
  movies: "M4 5h16v14H4zM8 5v14M16 5v14M4 9.5h4M4 14.5h4M16 9.5h4M16 14.5h4",
  shows: "M3.5 7h17v11h-17zM9 21h6M8.5 3.5 12 7l3.5-3.5",
  series: "M3.5 7h17v11h-17zM9 21h6M8.5 3.5 12 7l3.5-3.5",
  anime: "M12 4l2.3 4.9 5.2.6-3.9 3.6 1.1 5.3L12 15.7l-4.7 2.7 1.1-5.3-3.9-3.6 5.2-.6z",
  favourites: "M7 4h10v16l-5-4-5 4z",
  together: "M12 8a3 3 0 1 1-6 0 3 3 0 0 1 6 0zM3 20c0-3.3 2.7-6 6-6s6 2.7 6 6M15.5 5.2a3 3 0 0 1 0 5.6M17 14.2c2.4.6 4 2.9 4 5.8",
  albums: "M12 3.5a8.5 8.5 0 1 0 0 17 8.5 8.5 0 0 0 0-17zm0 6a2.5 2.5 0 1 0 0 5 2.5 2.5 0 0 0 0-5z",
  artists: "M12 4a4 4 0 1 1 0 8 4 4 0 0 1 0-8zM4.5 20c0-3.6 3.4-5.5 7.5-5.5s7.5 1.9 7.5 5.5",
  songs: "M9 18V6l10-2v12M9 18a3 3 0 1 1-6 0 3 3 0 0 1 6 0zM19 16a3 3 0 1 1-6 0 3 3 0 0 1 6 0z",
  music: "M9 18V6l10-2v12M9 18a3 3 0 1 1-6 0 3 3 0 0 1 6 0zM19 16a3 3 0 1 1-6 0 3 3 0 0 1 6 0z",
};

const MAIN = ["home", "movies", "shows", "favourites", "albums", "artists", "songs"];

function glyph(key) {
  if (!PATHS[key]) return icon("more");
  const svg = icon("more");
  svg.setAttribute("class", "lines");
  svg.firstChild.setAttribute("d", PATHS[key]);
  return svg;
}

// `e.said`, on a Manage service, is its health ({ level, text }), as the header shows it.
function link(e, route, inSheet) {
  return h(
    "a",
    { href: e.href, "aria-current": e.here(route) ? "page" : null },
    inSheet ? null : glyph(e.key),
    e.said && !inSheet ? h("span", { class: `tab-dot ${e.said.level}`, "aria-hidden": "true" }) : null,
    h("span", { class: "tab-name" }, e.name),
    e.said ? h("span", { class: inSheet ? "count-pill " + e.said.level : "visually-hidden" }, inSheet ? e.said.text : `, ${e.said.text}`) : null,
  );
}

export function drawTabBar(entries, route, { hidden }) {
  bar.hidden = hidden || entries.length === 0;
  if (bar.hidden) {
    if (sheet.open) sheet.close();
    return;
  }
  // Watch's Home, Movies, Shows and My List and Listen's Home, Albums, Artists and Songs; else
  // the first four. All of them when there are four or fewer.
  const main = entries.filter((e) => MAIN.includes(e.key));
  const picked = entries.length <= 4 ? entries : [...main, ...entries.filter((e) => !main.includes(e))].slice(0, 4);
  const shown = entries.filter((e) => picked.includes(e));
  const rest = entries.filter((e) => !picked.includes(e));
  const elsewhere = rest.some((e) => e.here(route));
  const more = rest.length
    ? h(
        "button",
        { type: "button", class: "tab-more", "aria-haspopup": "dialog", "aria-expanded": "false", "aria-current": elsewhere ? "page" : null, onclick: open },
        icon("more"),
        h("span", { class: "tab-name" }, "More"),
      )
    : null;
  bar.replaceChildren(h("ul", {}, [...shown.map((e) => link(e, route, false)), more].filter(Boolean).map((el) => h("li", {}, el))));
  sheet.replaceChildren(
    h("div", { class: "fold-head" }, h("h2", { class: "sidebar-title", id: "tab-more-title" }, "More"), h("button", { type: "button", class: "quiet", onclick: () => sheet.close() }, "Close")),
    h("ul", {}, rest.map((e) => h("li", {}, link(e, route, true)))),
  );
}

// Closed by Escape, Close or a tap around it, focus goes back to More; by a link followed, the
// page it opens takes it.
let followed = false;
function open(event) {
  const button = event.currentTarget;
  button.setAttribute("aria-expanded", "true");
  followed = false;
  sheet.showModal();
  sheet.addEventListener(
    "close",
    () => {
      button.setAttribute("aria-expanded", "false");
      if (!followed && button.isConnected) button.focus();
    },
    { once: true },
  );
}

sheet.addEventListener("click", (event) => {
  followed = Boolean(event.target.closest("a"));
  if (followed || event.target === sheet) sheet.close();
});

// The bar's height, so the page, the music bar and what sits on them leave room for it.
new ResizeObserver(() => {
  document.documentElement.style.setProperty("--tab-bar-height", `${bar.hidden ? 0 : bar.offsetHeight}px`);
}).observe(bar);
