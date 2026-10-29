// The overview of Manage → Requests: how things stand there, each line with where to see
// more. What needs attention comes first, from the admin health. System's is its Status
// (system-status.js).

import { group, when } from "/web/forms.js";
import { api, h, plural } from "/web/lib.js";

function fact(label, value, href, more) {
  return h("li", {}, h("span", { class: "fact-label" }, label), h("span", { class: "fact-value" }, value), href ? h("a", { href }, more) : null);
}

const DAY = 86_400;
const KINDS = { movie: "movies", series: "series", artist: "artists", album: "albums", song: "songs" };

export async function requestsOverview(ctx, attention) {
  const [page, imports, settings] = await Promise.all([
    api("/requests?all=true&limit=200"),
    api("/requests/imports?all=true"),
    api("/requests/settings"),
  ]);
  if (!ctx.current()) return [];
  const now = Date.now() / 1000;
  const waiting = page.items.filter((r) => r.state === "requested");
  const week = page.items.filter((r) => r.decided_at && now - r.decided_at < 7 * DAY);
  const approved = week.filter((r) => r.state === "approved").length;
  const declined = week.length - approved;
  const importsWaiting = imports.filter((i) => i.waiting > 0);
  const straight = settings.users.filter((u) => !u.is_admin && u.requests_approved).length;
  const others = settings.users.filter((u) => !u.is_admin).length;
  const unset = settings.defaults.filter((d) => d.library_id === null).map((d) => KINDS[d.kind] ?? d.kind);
  const oldest = waiting.reduce((min, r) => Math.min(min, r.created_at), Infinity);

  const facts = h(
    "ul",
    { class: "glance" },
    fact(
      "Waiting",
      waiting.length ? `${plural(waiting.length, "request", "requests")}, the oldest from ${when(oldest)}.` : "Nothing waits for a decision.",
      "#/admin/requests/decide",
      waiting.length ? "Decide" : "See requests",
    ),
    fact("Last 7 days", week.length ? `${approved} approved, ${declined} declined.` : "Nothing decided.", null, null),
    fact(
      "Playlist imports",
      importsWaiting.length
        ? `${plural(importsWaiting.length, "import has", "imports have")} songs waiting for a decision.`
        : imports.length
          ? `${plural(imports.length, "import", "imports")}, none waiting.`
          : "None yet.",
      "#/admin/requests/imports",
      "See imports",
    ),
    fact(
      "Where they go",
      `${others ? `${straight} of ${others} ${others === 1 ? "person skips" : "people skip"} approval` : "No one besides admins asks"}${unset.length ? `; no library chosen yet for ${unset.join(", ")}` : ""}.`,
      "#/admin/requests/settings",
      "Change",
    ),
  );
  return [attention, group("At a glance", facts)];
}
