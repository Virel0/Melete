// Manage → System → Libraries: rescanning one title's folder, for a file just copied in by hand,
// without scanning its whole library. A title is found by name among the movies and series.

import { field, group } from "/web/forms.js";
import { SignedOut, announce, api, h } from "/web/lib.js";

const FOUND = 6;

export function titleRescan(ctx) {
  const name = field("rescan-title", "Title", h("input", { type: "search", autocomplete: "off", spellcheck: "false" }), "A movie or series in a library, by its name.");
  const results = h("div", { class: "rescan-results" });
  const button = h("button", { type: "submit", class: "quiet" }, "Find it");
  const find = h("form", { class: "search-row", novalidate: true, role: "search", "aria-label": "Find a title to rescan" }, name.box, button);
  find.addEventListener("submit", async (event) => {
    event.preventDefault();
    name.clear();
    const q = name.input.value.trim();
    if (q.length < 2) {
      name.fail("Type at least two letters of its name.");
      name.input.focus();
      return;
    }
    button.disabled = true;
    results.replaceChildren(h("p", { class: "note", role: "status" }, "Looking…"));
    try {
      const query = new URLSearchParams({ q, limit: String(FOUND) });
      const [movies, series] = await Promise.all([api(`/movies?${query}`), api(`/series?${query}`)]);
      const titles = [
        ...movies.items.map((m) => ({ ...m, kind: "movies", noun: "Movie" })),
        ...series.items.map((s) => ({ ...s, kind: "series", noun: "Series" })),
      ];
      results.replaceChildren(
        titles.length
          ? h("ul", { class: "rows" }, titles.map((t) => titleRow(ctx, t)))
          : h("p", { class: "status", role: "status" }, `No movie or series called “${q}”. A folder not scanned yet isn't a title: scan its library instead.`),
      );
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      results.replaceChildren(h("p", { class: "error", role: "alert" }, err.message));
    } finally {
      button.disabled = false;
    }
  });
  return group(
    "Rescan a title's folder",
    h("p", { class: "note" }, "Scans that folder alone, then matches and reads what's new in it; the rest of its library, and its disk, are left alone."),
    find,
    results,
  );
}

function titleRow(ctx, t) {
  const label = t.year ? `${t.title} (${t.year})` : t.title;
  const said = h("p", { class: "note", role: "status" });
  const rescan = h("button", { type: "button", class: "quiet", "aria-label": `Rescan the folder of ${label}` }, "Rescan its folder");
  rescan.addEventListener("click", async () => {
    rescan.disabled = true;
    try {
      await api(`/${t.kind}/${t.id}/rescan`, { method: "POST" });
      said.textContent = "Asked for. What it finds shows on its page in a moment.";
      announce(`Rescan of ${label} asked for`);
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      said.textContent = err.message;
      said.classList.add("error");
    } finally {
      rescan.disabled = false;
    }
  });
  return h(
    "li",
    {},
    h("div", {}, h("a", { href: `#/${t.kind}/${t.id}` }, label), " ", h("span", { class: "badge" }, t.noun), said),
    h("div", { class: "row-actions" }, rescan),
  );
}
