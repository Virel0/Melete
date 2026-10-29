// Settings for where releases and wanted titles come from: indexers, quality profiles, import
// lists, and imports from Sonarr and Radarr. Admins only.
// This file has the indexers; the rest is in sources-profiles.js, sources-lists.js and
// sources-arrs.js.

import { confirmInline, field, form, group } from "/web/forms.js";
import { SignedOut, announce, api, h, plural } from "/web/lib.js";
import { arrBody, arrFields } from "/web/sources-arrs.js";

// Settings imports these pages from here.
export { arrs } from "/web/sources-arrs.js";
export { lists } from "/web/sources-lists.js";
export { profiles } from "/web/sources-profiles.js";

// The public trackers Melete reaches itself (src/indexers.rs PRESETS), by the name they're added
// under and how they're reached, which tells one already added even after it's renamed.
const PRESETS = [
  { key: "thepiratebay", name: "The Pirate Bay", kind: "apibay", about: "Movies and series, through its JSON API." },
  { key: "nyaa", name: "Nyaa", kind: "nyaa", about: "Anime with English subtitles, through its RSS feed." },
  { key: "animetosho", name: "AnimeTosho", kind: "torznab", about: "Anime, through its Torznab feed." },
  { key: "uindex", name: "Uindex", kind: "uindex", about: "Movies, series and anime; needs MELETE_FLARESOLVERR_URL on the server." },
];

const KINDS = { torznab: "Torznab", apibay: "JSON API", nyaa: "RSS", uindex: "Web page" };

function categories(list) {
  const has = (from) => list.some((c) => c >= from && c < from + 1000);
  const names = [has(2000) && "movies", has(5000) && "TV", list.includes(5070) && "anime"].filter(Boolean);
  return names.length ? `Has ${names.join(", ")}.` : "Lists no movie or TV categories.";
}

// ---------- Indexers ----------

export async function indexers(ctx) {
  const all = await api("/indexers");
  if (!ctx.current()) return [];
  const list = h("ul", { class: "rows" });
  const draw = (items) => list.replaceChildren(...items.map((ix) => indexerRow(ix, { ctx, refresh })));
  const refresh = async () => {
    const fresh = await api("/indexers");
    draw(fresh);
    adding.drawPresets(fresh);
  };
  const adding = addIndexers(ctx, all, () => refresh());
  // Drawn once `refresh` exists: each row holds on to it.
  draw(all);
  return [
    group("Indexers", all.length ? null : h("p", { class: "status" }, "No indexers yet. Add a public tracker or import Prowlarr's below."), list),
    ...adding.groups,
  ];
}

// One indexer's row: what it is, and testing, renaming, turning off and removing it. `extra`
// goes under its facts; `refresh` draws the list again after a change.
export function indexerRow(ix, { ctx, refresh, extra = null }) {
  const result = h("p", { class: "note", role: "status" });
  const actions = h("div", { class: "row-actions" });
  const failed = (err) => {
    if (err instanceof SignedOut) return ctx.signedOut();
    result.textContent = err.message;
    result.classList.add("error");
  };
  const change = (body) => api(`/indexers/${ix.id}`, { method: "PUT", body: { name: ix.name, enabled: ix.enabled, ...body } });
  const test = h("button", { type: "button", class: "quiet", "aria-label": `Test ${ix.name}` }, "Test");
  test.addEventListener("click", async () => {
    test.disabled = true;
    result.textContent = "Asking…";
    result.classList.remove("error");
    try {
      const answer = await api(`/indexers/${ix.id}/test`, { method: "POST" });
      result.textContent = `Answered: ${plural(answer.recent_releases, "release", "releases")} in its newest list. ${categories(answer.categories)}`;
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      result.textContent = err.message;
      result.classList.add("error");
    } finally {
      test.disabled = false;
    }
  });
  const toggle = h("button", { type: "button", class: "quiet", "aria-label": `${ix.enabled ? "Turn off" : "Turn on"} ${ix.name}` }, ix.enabled ? "Turn off" : "Turn on");
  toggle.addEventListener("click", async () => {
    toggle.disabled = true;
    result.classList.remove("error");
    try {
      await change({ enabled: !ix.enabled });
      announce(ix.enabled ? `Turned off ${ix.name}` : `Turned on ${ix.name}`);
      await refresh();
    } catch (err) {
      failed(err);
      toggle.disabled = false;
    }
  });
  const name = h("strong", {}, ix.name);
  const rename = h("button", { type: "button", class: "quiet", "aria-label": `Rename ${ix.name}` }, "Rename");
  rename.addEventListener("click", () => {
    const input = field(`indexer-${ix.id}-name`, "New name", h("input", { value: ix.name, autocomplete: "off", maxlength: "100" }));
    const save = form([input], "Save name", async () => {
      const wanted = input.input.value.trim();
      if (!wanted) {
        input.fail("Enter a name.");
        return null;
      }
      await change({ name: wanted });
      announce(`Renamed ${ix.name} to ${wanted}`);
      await refresh();
      return null;
    });
    const cancel = h("button", { type: "button", class: "quiet" }, "Cancel");
    cancel.addEventListener("click", () => {
      save.replaceWith(name);
      rename.focus();
    });
    save.querySelector(".form-actions").append(cancel);
    name.replaceWith(save);
    input.input.select();
  });
  const remove = h("button", { type: "button", class: "quiet", "aria-label": `Remove ${ix.name}` }, "Remove");
  remove.addEventListener("click", () =>
    confirmInline(actions, {
      question:
        ix.source === "prowlarr"
          ? `Remove “${ix.name}”? Searches stop asking it. Past searches keep what it found, and those results can still be grabbed. Importing from Prowlarr adds it back while Prowlarr has it; to stop using it for good, turn it off instead.`
          : `Remove “${ix.name}”? Searches stop asking it. Past searches keep what it found, and those results can still be grabbed. You can add it again under Public trackers.`,
      confirm: `Remove ${ix.name}`,
      run: async () => {
        await api(`/indexers/${ix.id}`, { method: "DELETE" });
        announce(`Removed ${ix.name}`);
        await refresh();
      },
    }),
  );
  actions.append(test, rename, toggle, remove);
  return h(
    "li",
    {},
    h(
      "div",
      {},
      name,
      " ",
      h("span", { class: "badge" }, ix.source === "prowlarr" ? "From Prowlarr" : KINDS[ix.kind]),
      ix.enabled ? null : h("span", { class: "badge" }, "Turned off"),
      h("p", { class: "note" }, categories(ix.categories)),
      extra,
      result,
    ),
    actions,
  );
}

// Adding indexers: the public trackers Melete reaches itself, and importing Prowlarr's.
// `refresh` draws the indexers again after one is added.
export function addIndexers(ctx, items, refresh) {
  const presets = h("ul", { class: "rows" });
  const drawPresets = (items) => {
    presets.replaceChildren(
      ...PRESETS.map((p) => {
        const added = items.some((ix) => ix.source === "melete" && ix.kind === p.kind);
        const result = h("p", { class: "note", role: "status" });
        const add = h("button", { type: "button", class: "quiet" }, `Add ${p.name}`);
        add.addEventListener("click", async () => {
          add.disabled = true;
          try {
            await api(`/indexers/presets/${p.key}`, { method: "POST" });
            announce(`Added ${p.name}`);
            await refresh();
          } catch (err) {
            if (err instanceof SignedOut) return ctx.signedOut();
            result.textContent = err.message;
            result.classList.add("error");
            add.disabled = false;
          }
        });
        return h(
          "li",
          {},
          h("div", {}, h("strong", {}, p.name), h("p", { class: "note" }, p.about), result),
          h("div", { class: "row-actions" }, added ? h("span", { class: "badge" }, "Added") : add),
        );
      }),
    );
  };
  drawPresets(items);

  const prowlarr = arrFields("prowlarr", "Prowlarr", 9696);
  const prowlarrForm = form(prowlarr, "Import from Prowlarr", async () => {
    const body = arrBody(prowlarr);
    if (!body) return null;
    const report = await api("/import/prowlarr", { method: "POST", body });
    prowlarr[1].input.value = "";
    await refresh();
    return `Imported ${plural(report.indexers, "torrent indexer", "torrent indexers")}${report.removed ? `, removed ${report.removed} Prowlarr no longer has` : ""}${report.skipped_usenet ? `; skipped ${report.skipped_usenet} Usenet` : ""}.`;
  });

  // Any Torznab feed, added by hand: asked what it can do before it's kept.
  const own = [
    field("own-name", "Name", h("input", { type: "text", autocomplete: "off", maxlength: "100" }), "What it's called in Melete."),
    field("own-url", "Torznab address", h("input", { type: "url", autocomplete: "off", spellcheck: "false", placeholder: "http://192.168.1.10:9117/api/v2.0/indexers/all/results/torznab/api" }), "As the server reaches it, without the key."),
    field("own-key", "API key", h("input", { type: "password", autocomplete: "off", spellcheck: "false" }), "Kept for searches and never shown again. Leave it empty for a feed that needs none."),
  ];
  const ownForm = form(own, "Test and add", async () => {
    const [name, url, key] = own;
    const body = { name: name.input.value.trim(), url: url.input.value.trim(), api_key: key.input.value.trim() };
    if (!body.name) return name.fail("Enter a name.");
    if (!/^https?:\/\/.+/.test(body.url)) return url.fail("Enter an address starting with http:// or https://.");
    const added = await api("/indexers", { method: "POST", body });
    for (const f of own) f.input.value = "";
    await refresh();
    return `Added ${added.name}: it answered with ${plural(added.categories.length, "category", "categories")}.`;
  });

  return {
    drawPresets,
    groups: [
      group("Public trackers", h("p", { class: "note" }, "Reached by Melete itself, without Prowlarr."), presets),
      group(
        "Your own Torznab indexer",
        h("p", { class: "note" }, "Any Torznab feed, such as Jackett or a tracker's own. Searches send it what they look for and its key; like every outside request, they follow the proxy settings."),
        ownForm,
      ),
      group("Prowlarr", h("p", { class: "note" }, "Its torrent indexers, searched through Prowlarr. Importing again updates them and removes ones Prowlarr no longer has; names given and indexers turned off here are kept."), prowlarrForm),
    ],
  };
}
