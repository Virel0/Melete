// RSS syncs, for admins: the latest syncs, what each named and grabbed, each search's decisions
// on demand, and syncing now.

import { group, when } from "/web/forms.js";
import { SignedOut, api, h, plural, seasonName } from "/web/lib.js";
import { searchResults } from "/web/wanted.js";

export async function rss(ctx) {
  const [syncs, movies, series] = await Promise.all([api("/rss/syncs?limit=20"), api("/wanted/movies"), api("/wanted/series")]);
  if (!ctx.current()) return [];
  const titleOf = (s) => {
    const w = s.wanted_movie_id ? movies.find((m) => m.id === s.wanted_movie_id) : series.find((x) => x.id === s.wanted_series_id);
    const name = w?.title ?? "A title no longer wanted";
    if (s.season === null) return name;
    return `${name}, ${s.episode !== null ? `${seasonName(s.season)} episode ${s.episode}` : `${seasonName(s.season)} pack`}`;
  };
  const syncRow = (s) => {
    const failed = s.indexers.filter((i) => i.error);
    return h(
      "li",
      {},
      h(
        "div",
        {},
        h("strong", {}, when(s.synced_at)),
        h("p", {}, `${plural(s.new_releases, "new release", "new releases")}, ${plural(s.searches.length, "wanted title", "wanted titles")} named, ${plural(s.grabbed.filter((g) => g.download_id).length, "grab", "grabs")}.`),
        failed.length ? h("p", { class: "error" }, failed.map((i) => `${i.name}: ${i.error}`).join(". ")) : null,
        s.grabbed.length
          ? h("ul", { class: "plain" }, s.grabbed.map((g) => h("li", { class: "note" }, `${g.refused ? "Not grabbed" : g.upgrade ? "Grabbed as an upgrade" : "Grabbed"}: ${g.title}${g.refused ? ` (${g.refused})` : ""}`)))
          : null,
        s.searches.length
          ? h(
              "ul",
              { class: "plain" },
              s.searches.map((x) => {
                const details = h("div");
                const open = h("button", { type: "button", class: "link", "aria-expanded": "false" }, `${titleOf(x)}: ${x.accepted} of ${plural(x.releases, "release", "releases")} accepted`);
                open.addEventListener("click", async () => {
                  if (open.getAttribute("aria-expanded") === "true") {
                    details.replaceChildren();
                    open.setAttribute("aria-expanded", "false");
                    return;
                  }
                  open.setAttribute("aria-expanded", "true");
                  details.replaceChildren(h("p", { class: "status" }, "Loading the decisions…"));
                  try {
                    details.replaceChildren(searchResults(await api(`/searches/${x.search_id}`), ctx));
                  } catch (err) {
                    if (err instanceof SignedOut) return ctx.signedOut();
                    details.replaceChildren(h("p", { class: "error" }, err.message));
                  }
                });
                return h("li", {}, open, details);
              }),
            )
          : null,
      ),
    );
  };
  const list = h("ul", { class: "rows" }, syncs.map(syncRow));
  const now = h("button", { type: "button" }, "Sync now");
  const status = h("p", { class: "form-result", role: "status" });
  now.addEventListener("click", async () => {
    now.disabled = true;
    status.textContent = "Asking every indexer for its newest releases…";
    try {
      const s = await api("/rss/sync", { method: "POST" });
      ctx.reload(`Synced: ${plural(s.new_releases, "new release", "new releases")}, ${plural(s.searches.length, "wanted title", "wanted titles")} named.`);
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      status.textContent = err.message;
      now.disabled = false;
    }
  });
  return [
    group(
      "RSS syncs",
      h("p", { class: "note" }, "Every 15 minutes each indexer's newest releases are checked against the wanted list; missing titles are grabbed when downloads are on. Kept 30 days; the latest 20 are shown."),
      h("div", { class: "form-actions" }, now, status),
      syncs.length ? list : h("p", { class: "status" }, "No syncs yet."),
    ),
  ];
}
