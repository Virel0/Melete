// The sidebar Listen and each Manage service have: a column at the left of the page at 1100 px
// and wider; narrower, a menu button above the page that opens the same list (under the button,
// or as a sheet from the foot of the screen on a phone). shell.js places it for each page.
//
// placeSidebar(def) draws it, or hides it when `def` is null. A definition is
//   { label, title, groups: [{ name, inHeader, items: [{ href, name, count, here }] }], more }
// - label names the list for screen readers ("Movies pages"); title heads it ("Movies").
// - Each group may have a name, shown as a small heading over its items.
// - inHeader: the header already lists this group's items, so the menu button leaves them out
//   (and isn't shown at all when every group is in the header, as with Listen's sections).
// - An item's `here` marks it as the page on screen; `count` is a number shown beside it.
// - `more`, when given, is a promise of further groups (playlists, say): they're added once
//   they come, and an answer that fails leaves the groups already given.
// Names too long for the column are cut with "…" and shown in full on hover.

import { h } from "/web/lib.js";
import { clip } from "/web/page-parts.js";

const frame = document.getElementById("frame");
const column = document.getElementById("sidebar");
const fold = document.getElementById("fold");
const toggle = document.getElementById("fold-toggle");
const menu = document.getElementById("fold-menu");
const phone = matchMedia("(max-width: 40rem)");

let drawn = 0;

function list(def, groups) {
  return groups.map((g, i) =>
    h(
      "div",
      { class: "sidebar-group" },
      g.name ? h("h2", { class: "sidebar-heading", id: `${def.id}-${i}` }, g.name) : null,
      h(
        "ul",
        { "aria-labelledby": g.name ? `${def.id}-${i}` : null },
        g.items.map((item) =>
          h(
            "li",
            {},
            h(
              "a",
              { href: item.href, "aria-current": item.here ? "page" : null },
              clip(item.name),
              item.count === undefined || item.count === null ? null : h("span", { class: "count" }, String(item.count)),
            ),
          ),
        ),
      ),
    ),
  );
}

function draw(def, groups) {
  column.setAttribute("aria-label", def.label);
  column.replaceChildren(h("p", { class: "sidebar-title" }, def.title), ...list({ ...def, id: "sidebar" }, groups));
  const folded = groups.filter((g) => !g.inHeader);
  fold.hidden = folded.length === 0;
  const here = folded.flatMap((g) => g.items).find((i) => i.here);
  toggle.replaceChildren(h("span", { class: "clip" }, here ? `${def.title}: ${here.name}` : def.title));
  menu.replaceChildren(
    h(
      "div",
      { class: "fold-head" },
      h("h2", { class: "sidebar-title", id: "fold-title" }, def.title),
      h("button", { type: "button", class: "quiet", onclick: () => closeMenu() }, "Close"),
    ),
    h("nav", { "aria-label": def.label }, list({ ...def, id: "fold" }, folded)),
  );
}

export function placeSidebar(def) {
  const mine = ++drawn;
  closeMenu({ refocus: false });
  frame.classList.toggle("with-sidebar", Boolean(def));
  column.hidden = !def;
  if (!def) {
    fold.hidden = true;
    return;
  }
  draw(def, def.groups);
  def.more?.then((more) => mine === drawn && draw(def, [...def.groups, ...more])).catch(() => {});
}

function openMenu() {
  toggle.setAttribute("aria-expanded", "true");
  // A sheet over the page on a phone, with focus kept in it; else a list under the button.
  if (phone.matches) menu.showModal();
  else menu.show();
  (menu.querySelector('[aria-current="page"]') ?? menu.querySelector("a"))?.focus();
}

function closeMenu({ refocus = true } = {}) {
  if (!menu.open) return;
  const inside = menu.contains(document.activeElement);
  menu.close();
  toggle.setAttribute("aria-expanded", "false");
  if (refocus && inside) toggle.focus();
}

toggle.addEventListener("click", () => (menu.open ? closeMenu() : openMenu()));
// The sheet's own Escape closes it as a modal does; the list under the button closes the same
// way, and on a click anywhere else or a link followed.
menu.addEventListener("close", () => toggle.setAttribute("aria-expanded", "false"));
menu.addEventListener("click", (event) => {
  if (event.target.closest("a") || event.target === menu) closeMenu({ refocus: false });
});
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && menu.open) closeMenu();
});
document.addEventListener("click", (event) => {
  if (menu.open && !fold.contains(event.target)) closeMenu({ refocus: false });
});
