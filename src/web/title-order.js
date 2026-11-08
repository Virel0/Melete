// The order a series' seasons and episodes follow, under its page's header: said when it's one
// of TMDB's episode groups rather than TMDB's own; and for admins, when the files use seasons
// TMDB's own order doesn't have, the group that comes closest, with Use this order. Other
// orders are chosen on the series' Manage page.

import { SignedOut, announce, api, h } from "/web/lib.js";
import { signIn } from "/web/page-state.js";

// `manage` is the series' Manage page; `redraw` draws this page again.
export function orderNote(s, admin, manage, redraw) {
  const o = s.episode_order;
  if (!o) return null;
  const change = admin ? [" ", h("a", { href: manage }, "Change")] : [];
  if (o.group_id) {
    const why = o.chosen_by === "auto" ? "chosen because the files' seasons fit it exactly" : "an admin's choice";
    return h("p", { class: "note" }, `Seasons and episodes follow TMDB's “${o.name}” episode group, ${why}.`, ...change);
  }
  if (!o.suggested || !admin) return null;
  const fits = o.suggested.exact ? "has exactly their seasons and episodes" : "comes closest to them";
  const said = h("span", { class: "form-result", role: "status" });
  const use = h("button", { type: "button", class: "quiet" }, "Use this order");
  use.addEventListener("click", async () => {
    use.disabled = true;
    said.textContent = "";
    try {
      await api(`/series/${s.id}/order`, { method: "PUT", body: { group_id: o.suggested.group_id } });
      announce(`Seasons and episodes follow TMDB's “${o.suggested.name}” episode group now.`);
      redraw();
    } catch (err) {
      if (err instanceof SignedOut) return signIn("Your session ended. Sign in again.");
      said.textContent = err.message;
      use.disabled = false;
    }
  });
  return h(
    "div",
    { class: "note order-note" },
    h("p", {}, `This series' files use seasons TMDB's own order doesn't have, so some show as episodes TMDB doesn't know. TMDB's “${o.suggested.name}” episode group ${fits}. Nothing on disk is renamed or moved.`),
    h("p", {}, use, " ", h("a", { href: manage }, "Other orders"), " ", said),
  );
}
