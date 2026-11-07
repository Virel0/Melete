// Watch's pages follow the server's live updates (`GET /api/v1/events`), so a title just
// imported, or put on My List on another device, shows without a reload. Each kind comes once
// on connecting, which only says where things stand, then again whenever it changes.
//
// - followWatch(gen, { library, favourites }): calls `library()` when a title comes or goes and
//   `favourites()` when My List changes, until the page is left. The browser reconnects a
//   dropped stream by itself; one it gives up on is opened again a little later, unless the
//   session has ended, which goes back to sign-in.
// - keepPlace(box, nodes): puts `nodes` in `box` in place of what's there, keeping the
//   keyboard's focus on the same link or button (the same title in the same row) and each
//   row's sideways scroll, so a redraw doesn't lose anyone's place.

import { SignedOut, api } from "/web/lib.js";
import { generation, leaving, signIn } from "/web/page-state.js";

const RETRY_MS = 15_000;
// How soon after connecting the server has sent where things stand.
const FIRST_MS = 1500;

export function followWatch(gen, on) {
  const last = {};
  let source = null;
  let retry = null;
  const open = () => {
    if (gen !== generation) return;
    source = new EventSource("/api/v1/events");
    let opened = Date.now();
    source.addEventListener("open", () => (opened = Date.now()));
    for (const kind of ["library", "favourites"]) {
      source.addEventListener(kind, (message) => {
        let version;
        try {
          version = JSON.parse(message.data).version;
        } catch {
          return;
        }
        const before = last[kind];
        last[kind] = version;
        // A kind first heard well after connecting is new since (My List's first title,
        // which isn't sent before there's one), not where things stood.
        const changed = before === undefined ? Date.now() - opened > FIRST_MS : before !== version;
        if (changed && gen === generation) on[kind]?.();
      });
    }
    source.addEventListener("error", async () => {
      if (source.readyState !== EventSource.CLOSED) return;
      try {
        await api("/me");
      } catch (err) {
        if (err instanceof SignedOut) return signIn("Your session ended. Sign in again.");
      }
      if (gen === generation) retry = setTimeout(open, RETRY_MS);
    });
  };
  open();
  leaving.push(() => {
    clearTimeout(retry);
    source?.close();
  });
}

// Who a link or button is, to find it again after a redraw: what it is, where it leads (or
// where its list item's link does) and the row it's in.
function identity(el) {
  const href = el.getAttribute("href") ?? el.closest("li")?.querySelector("a[href]")?.getAttribute("href") ?? "";
  const row = el.closest("[aria-labelledby]")?.getAttribute("aria-labelledby") ?? "";
  return [el.tagName, el.className, href, row, el.tagName === "BUTTON" ? el.textContent : ""].join("|");
}

export function keepPlace(box, nodes) {
  const active = box.contains(document.activeElement) ? document.activeElement : null;
  const was = active ? identity(active) : null;
  const scrolled = new Map([...box.querySelectorAll(".watch-shelf")].map((s) => [s.getAttribute("aria-labelledby"), s.scrollLeft]));
  box.replaceChildren(...nodes);
  for (const shelf of box.querySelectorAll(".watch-shelf")) {
    const left = scrolled.get(shelf.getAttribute("aria-labelledby"));
    if (left) shelf.scrollLeft = left;
  }
  if (!was) return;
  const again = [...box.querySelectorAll("a[href], button")].find((el) => identity(el) === was);
  again?.focus({ preventScroll: true });
}
