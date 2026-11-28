// The frame every page sits in: the header with Melete, the Watch | Listen | Manage switch,
// the current side's entries (sides.js), Search (the command palette) and the account menu.
//
// - startShell(me) once signed in: finds what the header lists (the anime library, whether
//   this way in opens Manage) and shows it.
// - placeShell(route) before each page is drawn: marks the side and entry the address belongs
//   to, and remembers it as that side's last page, so the switch takes you back to it.
// - stopShell() on signing out.
//
// Each side keeps its last page for the session (sessionStorage, so a reload keeps it too);
// the current side's switch goes to its home. Manage shows only to admins, and only when the
// way in opens admin pages: the admin health answer is asked for at once, and Manage shows once
// it comes; a refusal leaves it out rather than showing a switch that leads to an error.
// Manage's entries are its services (manage-services.js), each with its health in words, kept
// up to date by the `melete:health` event, and each keeping its own last page.
//
// Listen and each Manage service also get the sidebar (sidebar.js): placeShell puts the right
// one in place for the page, or none on Watch.

import { SignedOut, api, h } from "/web/lib.js";
import { badgeText, currentHealth } from "/web/health.js";
import { SERVICES, findingsOf, placeOf, serviceSidebar } from "/web/manage-services.js";
import { forgetAlbumLikes } from "/web/music-album-likes.js";
import { forgetListenLists } from "/web/music-sidebar.js";
import { placeSidebar } from "/web/sidebar.js";
import { SIDES, listenEntries, listenSidebar, sideOf, watchEntries } from "/web/sides.js";
import { drawTabBar } from "/web/tab-bar.js";

const bar = document.getElementById("bar");
const sides = document.getElementById("sides");
const entries = document.getElementById("entries");
const STORE = "melete.last-pages";

let me = null;
let canManage = false;
let categories = [];
let found = null;
let side = "watch";
let route = { parts: [], params: new URLSearchParams() };
// The last page of each side, and of each Manage service: { watch: "#/movies", "manage/music": … }.
let last = {};

function load() {
  try {
    return JSON.parse(sessionStorage.getItem(STORE) ?? "{}") ?? {};
  } catch {
    return {};
  }
}

function save() {
  try {
    sessionStorage.setItem(STORE, JSON.stringify(last));
  } catch {
    // Storage turned off: the switch still works, it just forgets on reload.
  }
}

// Manage's entries: each service with its health in words.
function manageEntries() {
  return SERVICES.map((s) => ({
    key: s.key,
    name: s.name,
    href: last[`manage/${s.key}`] ?? `#/admin/${s.key}`,
    here: ({ parts }) => parts[0] === "admin" && placeOf(parts)?.service === s,
    health: findingsOf(s, found),
  }));
}

function entriesOf(key) {
  if (key === "listen") return listenEntries();
  if (key === "manage") return manageEntries();
  return watchEntries(categories);
}

// The count beside a Manage service: problems first, then what's waiting. The number shows;
// what it counts ("2 problems") is read out with it and shown on hover.
function countPill(said) {
  const [count, ...what] = said.text.split(" ");
  return h("span", { class: `count-pill ${said.level}`, title: said.text }, count, h("span", { class: "visually-hidden" }, ` ${what.join(" ")}`));
}

function draw() {
  for (const link of sides.querySelectorAll("[data-side]")) {
    const key = link.dataset.side;
    const info = SIDES.find((s) => s.key === key);
    if (key === "manage") link.hidden = !canManage;
    link.href = key === side ? info.home : last[key] ?? info.home;
    link.setAttribute("aria-current", String(key === side));
  }
  const info = SIDES.find((s) => s.key === side);
  entries.setAttribute("aria-label", side === "manage" ? "Manage services" : info.name);
  const listed = entriesOf(side).map((e) => ({ ...e, said: e.health ? badgeText(e.health) : null }));
  entries.replaceChildren(
    h(
      "ul",
      {},
      listed.map((e) =>
        h("li", {}, h("a", { href: e.href, "aria-current": e.here(route) ? "page" : null }, e.name, e.said ? countPill(e.said) : null)),
      ),
    ),
  );
  drawTabBar(listed, route, { hidden: bar.hidden || route.parts[0] === "play" });
  fit();
}

// The side's entries stay on the header's row only while all of them fit there; otherwise
// they all move to a row of their own under it (`stacked` in shell.css), which scrolls
// sideways to the current one. Measured with them on the row, before the page is painted.
function fit() {
  if (bar.hidden) return;
  bar.classList.remove("stacked");
  const list = entries.firstElementChild;
  if (list && list.scrollWidth > entries.clientWidth + 1) bar.classList.add("stacked");
  const here = entries.querySelector('[aria-current="page"]');
  if (here && bar.classList.contains("stacked")) {
    const left = here.offsetLeft - entries.offsetLeft;
    if (left < entries.scrollLeft || left + here.offsetWidth > entries.scrollLeft + entries.clientWidth) entries.scrollLeft = left;
  }
}
addEventListener("resize", fit);

// Sets the header up for `user`; pages can be drawn before what it asks for arrives.
export function startShell(user) {
  me = user;
  last = load();
  canManage = false;
  found = null;
  draw();
  fetchCategories();
  if (me.is_admin) {
    currentHealth(true)
      .then(() => true)
      .catch((err) => (err instanceof SignedOut ? null : err.status !== 403))
      .then((open) => {
        // A 403 means this way in doesn't open admin pages: no Manage switch. Any other
        // failure leaves it there, and the pages say what went wrong.
        if (open === null || me !== user) return;
        canManage = open;
        placeShell(route);
      });
  }
}

// The categories the video libraries use, for Watch's Shows and Anime.
async function fetchCategories() {
  const asked = me;
  try {
    const libraries = await api("/browse/libraries");
    if (asked !== me) return;
    categories = [...new Set(libraries.map((l) => l.category).filter(Boolean))];
    draw();
  } catch {
    // Video closed to this way in, or not answering: Watch lists its entries without Anime.
  }
}

// Marks where the address `here` is ({ parts, params }), and remembers it as its side's page.
export function placeShell(here) {
  route = here;
  const belongs = sideOf(here);
  if (belongs && (belongs !== "manage" || canManage)) {
    side = belongs;
    const href = location.hash || "#/";
    last[side] = href;
    const place = here.parts[0] === "admin" ? placeOf(here.parts) : null;
    if (side === "manage" && place) last[`manage/${place.service.key}`] = href;
    save();
  }
  draw();
  placeSidebar(sidebarOf(belongs === side ? side : null));
}

// The sidebar for the page on screen: Listen's, a Manage service's, or none.
function sidebarOf(key) {
  if (key === "listen") return listenSidebar(route);
  const place = key === "manage" && route.parts[0] === "admin" ? placeOf(route.parts) : null;
  return place ? serviceSidebar(place) : null;
}

// Signing out forgets the last pages too, so whoever signs in next in this tab starts afresh.
export function stopShell() {
  me = null;
  canManage = false;
  found = null;
  side = "watch";
  last = {};
  save();
  forgetAlbumLikes();
  forgetListenLists();
  placeSidebar(null);
}

// A page changed the pins or a playlist: Listen's sidebar lists them, so it's drawn again.
for (const name of ["melete:pins", "melete:playlists"]) {
  document.addEventListener(name, () => {
    if (me && side === "listen" && sideOf(route) === "listen") placeSidebar(sidebarOf("listen"));
  });
}

document.addEventListener("melete:health", (event) => {
  found = event.detail;
  if (me && side === "manage") draw();
});

// The header's height, for what sticks under it (sidebars, panels).
new ResizeObserver(() => {
  document.documentElement.style.setProperty("--header-height", `${bar.offsetHeight}px`);
}).observe(bar);
