// Trending: this week's movies and shows on TMDB, as posters, each to open when it's in the
// library or to ask for when it isn't. Only while discovery is on: the section asks
// /discover/status first (nothing goes to TMDB for that) and stays hidden while it's off, or
// when TMDB doesn't answer.
//
// trendingSection(ctx, { id, className, on }) → the section, hidden until something trending
// has come. `ctx` has signedOut(); `on` is discovery's state when the page knows it already.

import { SignedOut, api, h } from "/web/lib.js";
import { art } from "/web/cards.js";
import { hold, release } from "/web/page-parts.js";

const KINDS = [
  ["movie", "Movies"],
  ["series", "Shows"],
];

export function trendingSection(ctx, { id = "trending", className = "trending", on } = {}) {
  const section = h("section", { class: className, "aria-labelledby": `${id}-head`, hidden: true });
  fill(ctx, section, id, on);
  return section;
}

async function fill(ctx, section, id, known) {
  let charts;
  try {
    const on = known ?? (await api("/discover/status")).on;
    if (!on) return;
    charts = await Promise.all(KINDS.map(([kind]) => api(`/discover/trending?${new URLSearchParams({ kind })}`)));
  } catch (err) {
    if (err instanceof SignedOut) return ctx.signedOut();
    // Turned off since, or TMDB not answering: nothing to show, and nothing said.
    return;
  }
  const shelves = KINDS.map(([kind, name], i) => [kind, name, charts[i]]).filter(([, , items]) => items.length);
  if (!shelves.length) return;
  section.replaceChildren(
    h("h2", { id: `${id}-head` }, "Trending this week"),
    ...shelves.flatMap(([kind, name, items]) => [
      h("h3", { id: `${id}-${kind}` }, name),
      h("ul", { class: "grid shelf", "aria-labelledby": `${id}-${kind}` }, items.map((item) => trendingCard(ctx, item))),
    ]),
  );
  section.hidden = false;
}

// A trending title: its page when it's in the library; else its poster, and Ask for it, or
// that it's asked for already.
function trendingCard(ctx, item) {
  const facts = [h("div", { class: "title" }, item.title), item.year ? h("div", { class: "meta" }, String(item.year)) : null];
  if (item.state === "playable") {
    const href = `#/${item.kind === "movie" ? "movies" : "series"}/${item.library_id}`;
    return h("li", {}, h("a", { class: "card", href }, art(item.poster), facts, h("div", { class: "meta" }, "In the library")));
  }
  if (item.state === "requested") {
    const asked = item.request_id ? h("a", { href: "#/requests/mine" }, "You asked for this") : "Wanted";
    return h("li", {}, h("div", { class: "card" }, art(item.poster), facts, h("div", { class: "meta" }, asked)));
  }
  const card = h("div", { class: "card" }, art(item.poster), facts);
  const said = h("p", { class: "meta", role: "status" });
  const button = h("button", { type: "button", class: "quiet", "aria-label": `Ask for ${item.title}` }, "Ask for it");
  button.addEventListener("click", async () => {
    if (!hold(button)) return;
    said.textContent = "Asking…";
    try {
      const asked = await api("/requests", { method: "POST", body: { kind: item.kind, tmdb_id: item.tmdb_id } });
      said.textContent =
        asked.state === "approved" ? `Approved: ${asked.title} is on its way.` : `Asked for ${asked.title}. An admin will decide.`;
      // Focus goes to what took the button's place, not off the page.
      const mine = h("a", { href: "#/requests/mine" }, "See your requests");
      button.replaceWith(mine);
      mine.focus();
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      said.textContent = err.message;
      release(button);
    }
  });
  return h("li", {}, card, h("div", { class: "trending-ask" }, button, said));
}
