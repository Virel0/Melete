// The order a series' episodes follow, on its Manage page: TMDB's own, or one of the series'
// TMDB episode groups (the seasons other sites use, say). A choice here is final: Melete never
// changes it by itself. Nothing on disk is renamed or moved.

import { form } from "/web/forms.js";
import { SignedOut, announce, api, h, plural } from "/web/lib.js";
import { placeholders } from "/web/page-parts.js";

// What the series follows, for its facts: "TMDB's own", or the group and who chose it.
export function orderSaid(order) {
  if (!order) return null;
  if (!order.group_id && order.suggested) {
    const fits = order.suggested.exact ? "fits the files exactly" : "comes closer to the files";
    return `TMDB's own order; its “${order.suggested.name}” episode group ${fits}`;
  }
  if (!order.group_id) return order.chosen_by === "admin" ? "TMDB's own order, an admin's choice" : "TMDB's own order";
  const why = order.chosen_by === "auto" ? "chosen because the files' seasons fit it exactly" : "an admin's choice";
  return `TMDB's “${order.name}” episode group, ${why}`;
}

// "3 seasons of 24, 23 and 12 episodes".
function seasonsSaid(seasons) {
  if (!seasons.length) return "no seasons";
  const counts = seasons.map((s) => s.episodes);
  const list = counts.length > 1 ? `${counts.slice(0, -1).join(", ")} and ${counts.at(-1)}` : String(counts[0]);
  return `${plural(seasons.length, "season", "seasons")} of ${list} episodes`;
}

function choice(name, value, checked, label, hint) {
  const id = `order-${value || "tmdb"}`;
  const input = h("input", { type: "radio", name, id, value, checked, "aria-describedby": hint ? `${id}-hint` : null });
  return h("div", { class: "check" }, input, h("label", { for: id }, label, hint ? h("span", { class: "hint", id: `${id}-hint` }, ` · ${hint}`) : null));
}

// The panel: each order with how it fits the files, the one followed chosen.
export function orderPanel(ctx, seriesId) {
  const box = h("div", {}, placeholders("the episode orders", { kind: "lines" }));
  const panel = h(
    "section",
    { class: "order-box", "aria-labelledby": "order-title" },
    h("h2", { class: "part", id: "order-title", tabindex: "-1" }, "Episode order"),
    h("p", { class: "note" }, "Seasons and episodes everywhere follow the order chosen: browsing, the apps, Missing, searches and RSS. Nothing on disk is renamed or moved; monitoring, downloads and history move with their episodes. A choice here is final: Melete never changes it by itself."),
    box,
  );
  api(`/series/${seriesId}/orders`)
    .then((o) => ctx.current() && box.replaceChildren(choices(ctx, seriesId, o)))
    .catch((err) => {
      if (err instanceof SignedOut) return ctx.signedOut();
      box.replaceChildren(h("p", { class: "form-result error", role: "alert" }, err.message));
    });
  return panel;
}

function choices(ctx, seriesId, o) {
  const name = `order-${seriesId}`;
  const now = o.order.group_id ?? "";
  const options = [
    choice(name, "", now === "", "TMDB's own order", null),
    ...o.groups.map((g) => {
      const fit = g.exact ? "fits the files exactly" : `holds ${plural(g.covered, "of the files' episodes", "of the files' episodes")}`;
      const suggested = o.suggested?.group_id === g.group_id ? " · suggested" : "";
      return choice(name, g.group_id, now === g.group_id, g.name, `${seasonsSaid(g.seasons)} · ${fit}${suggested}`);
    }),
  ];
  const none = o.groups.length ? null : h("p", { class: "note" }, o.groups_fetched_at ? "TMDB has no other orders for this series." : "TMDB's other orders for this series aren't known yet: Melete asks for them while TMDB is set up.");
  const field = { box: h("fieldset", { class: "choices" }, h("legend", {}, "Follow"), options, none), clear() {} };
  return form([field], "Use this order", async () => {
    const picked = field.box.querySelector("input:checked")?.value ?? "";
    const done = await api(`/series/${seriesId}/order`, { method: "PUT", body: { group_id: picked || null } });
    const said = `Episodes follow ${orderSaid(done.order)}.`;
    announce(said);
    ctx.reload(said);
    return said;
  });
}
