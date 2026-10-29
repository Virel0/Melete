// Manage → Indexers, shared by Movies, Series, Anime and Music: the indexers in a table (what
// each is, who uses it, on or off, how it answered last; testing, renaming and removing one),
// adding one, and Soulseek. RSS syncs and the blocklist are in wanted-rss.js and
// wanted-set-aside.js; trying a release name is in release-try.js.

import { field, form, group, when } from "/web/forms.js";
import { SignedOut, announce, api, h, plural } from "/web/lib.js";
import { currentHealth, forgetHealth } from "/web/health.js";
import { emptyState, stateLabel } from "/web/page-parts.js";
import { ago, askFirst, attentionStrip, onOff, rowTable } from "/web/manage-parts.js";
import { addIndexers } from "/web/sources.js";
import { serviceSection } from "/web/services.js";

const HERE = "#/admin/indexers/list";

// How each is reached, in words: Prowlarr's feeds, a Torznab feed added by hand (or
// AnimeTosho's), and the presets Melete reaches itself.
function kindOf(ix) {
  if (ix.source === "prowlarr") return "From Prowlarr";
  return { torznab: "Torznab", apibay: "Preset (JSON API)", nyaa: "Preset (RSS feed)", uindex: "Preset (needs FlareSolverr)" }[ix.kind] ?? ix.kind;
}

// Which services search it, by the categories it has: movie searches ask those with movie
// categories, series and anime searches those with TV ones, album searches those with music.
export function usedBy(categories) {
  const has = (from) => categories.some((c) => c >= from && c < from + 1000);
  const users = [has(2000) && "Movies", has(5000) && "Series, Anime", has(3000) && "Music"].filter(Boolean);
  return users.length ? users.join(", ") : "Nothing: it has no movie, TV or music categories";
}

// The latest RSS sync's word on each indexer, by id.
async function lastRss() {
  const [sync] = await api("/rss/syncs?limit=1");
  return { sync, by: new Map((sync?.indexers ?? []).map((i) => [i.indexer_id, i])) };
}

function lastResult(ix, { sync, by }) {
  if (!ix.enabled) return stateLabel("off", "Off: not asked");
  const seen = by.get(ix.id);
  if (!sync || !seen) return h("span", { class: "note" }, "Not in an RSS sync yet");
  if (seen.error) return h("span", {}, stateLabel("failed", `Failed ${ago(sync.synced_at)}`), h("span", { class: "why" }, seen.error));
  return h("span", { title: when(sync.synced_at) }, `${plural(seen.results, "release", "releases")}, ${ago(sync.synced_at)}`);
}

// ---------- The list ----------

export async function indexerList(ctx) {
  const [all, rss, health] = await Promise.all([api("/indexers"), lastRss(), currentHealth(true)]);
  if (!ctx.current()) return [];
  const page = { ctx, rss, items: all, strip: h("div"), table: h("div"), status: h("p", { class: "form-result", role: "status" }) };
  page.strip.replaceChildren(attentionStrip(health.indexers, HERE) ?? "");
  page.refresh = async (focusOn) => {
    forgetHealth();
    const [fresh, found] = await Promise.all([api("/indexers"), currentHealth(true)]);
    page.items = fresh;
    page.strip.replaceChildren(attentionStrip(found.indexers, HERE) ?? "");
    draw(page);
    if (focusOn) page.table.querySelector(focusOn)?.focus();
  };
  draw(page);
  const add = h("a", { class: "button", href: "#/admin/indexers/add" }, "Add an indexer");
  return [
    page.strip,
    h("div", { class: "toolbar" }, add, all.length ? testAll(page) : null, page.status),
    h("p", { class: "note" }, "Searches and RSS syncs ask every indexer that's turned on. Turning one off keeps it, its name and its history; removing it keeps the results it already found."),
    page.table,
  ];
}

function draw(page) {
  if (!page.items.length) {
    page.table.replaceChildren(emptyState("No indexers yet, so Melete can't search for anything.", { href: "#/admin/indexers/add", label: "Add an indexer" }));
    return;
  }
  page.table.replaceChildren(rowTable(["Name", "Kind", "Used by", "On", "Last result", ""], page.items.map((ix) => indexerRow(page, ix))));
}

// Testing the ones that failed (or every one turned on when none did), one after another, so
// the trackers aren't all asked at the same moment.
function testAll(page) {
  const failing = () => page.items.filter((i) => i.enabled && page.rss.by.get(i.id)?.error);
  const n = failing().length;
  const button = h("button", { type: "button", class: "quiet" }, n === 0 ? "Test all" : n === 1 ? "Test the failing one" : `Test the ${n} failing ones`);
  button.addEventListener("click", async () => {
    const which = failing().length ? failing() : page.items.filter((i) => i.enabled);
    button.disabled = true;
    const outcomes = [];
    for (const [i, ix] of which.entries()) {
      page.status.textContent = `Testing ${ix.name} (${i + 1} of ${which.length})…`;
      try {
        const answer = await api(`/indexers/${ix.id}/test`, { method: "POST" });
        outcomes.push(`${ix.name} answered ${plural(answer.recent_releases, "release", "releases")}`);
      } catch (err) {
        if (err instanceof SignedOut) return page.ctx.signedOut();
        outcomes.push(`${ix.name} failed: ${err.message}`);
      }
    }
    page.status.textContent = outcomes.length ? `${outcomes.join(". ")}.` : "No indexer is turned on to test.";
    button.disabled = false;
  });
  return button;
}

// One indexer's row, and under it the room for what its buttons open.
function indexerRow(page, ix) {
  const { ctx } = page;
  const more = h("div", { class: "row-room" });
  const say = (text, bad = false) => more.replaceChildren(h("p", { class: bad ? "error" : "note", role: bad ? "alert" : "status" }, text));
  const failed = (err) => {
    if (err instanceof SignedOut) return ctx.signedOut();
    say(err.message, true);
  };
  const change = (body) => api(`/indexers/${ix.id}`, { method: "PUT", body: { name: ix.name, enabled: ix.enabled, ...body } });
  const button = (label, name, onclick) =>
    h("button", { type: "button", class: "quiet", "aria-label": `${label.replace("…", "")} ${ix.name}`, "data-do": `${name}-${ix.id}`, onclick }, label);

  const test = button("Test", "test", async () => {
    test.disabled = true;
    say("Asking…");
    try {
      const answer = await api(`/indexers/${ix.id}/test`, { method: "POST" });
      say(`${ix.name} answered: ${plural(answer.recent_releases, "release", "releases")} in its newest list. Used by ${usedBy(answer.categories)}.`);
    } catch (err) {
      failed(err);
    } finally {
      test.disabled = false;
    }
  });
  const rename = button("Rename", "rename", () => renaming(page, ix, more, rename, change));
  const remove = button("Remove…", "remove", () =>
    askFirst(more, {
      question:
        ix.source === "prowlarr"
          ? `Remove “${ix.name}”? Searches stop asking it. Past searches keep what it found, and those results can still be grabbed. Importing from Prowlarr adds it back while Prowlarr has it; to stop using it for good, turn it off instead.`
          : `Remove “${ix.name}”? Searches stop asking it. Past searches keep what it found, and those results can still be grabbed. You can add it again under Add an indexer.`,
      confirm: `Remove ${ix.name}`,
      keep: "Keep it",
      back: remove,
      signedOut: ctx.signedOut,
      run: async () => {
        await api(`/indexers/${ix.id}`, { method: "DELETE" });
        announce(`Removed ${ix.name}`);
        await page.refresh("[data-do^=test], a.button");
      },
    }),
  );
  const toggle = onOff({
    on: ix.enabled,
    label: `${ix.name} searched`,
    change: async (on) => {
      try {
        await change({ enabled: on });
      } catch (err) {
        failed(err);
        return false;
      }
      announce(on ? `Turned on ${ix.name}` : `Turned off ${ix.name}`);
      await page.refresh(`[data-do="test-${ix.id}"]`).catch(failed);
      page.table.querySelector(`[role=switch][aria-label="${CSS.escape(`${ix.name} searched`)}"]`)?.focus();
      return true;
    },
  });
  return {
    cells: [
      h("span", { class: "name" }, ix.name),
      kindOf(ix),
      usedBy(ix.categories),
      toggle,
      lastResult(ix, page.rss),
      h("div", { class: "row-actions" }, test, rename, remove),
    ],
    more,
  };
}

// Renaming in the room under the row; Cancel or Escape puts focus back on Rename.
function renaming(page, ix, more, rename, change) {
  const input = field(`indexer-${ix.id}-name`, `New name for ${ix.name}`, h("input", { value: ix.name, autocomplete: "off", maxlength: "100" }));
  const back = () => {
    more.replaceChildren();
    rename.focus();
  };
  const save = form([input], "Save name", async () => {
    const wanted = input.input.value.trim();
    if (!wanted) {
      input.fail("Enter a name.");
      return null;
    }
    if (wanted === ix.name) {
      back();
      return null;
    }
    await change({ name: wanted });
    announce(`Renamed ${ix.name} to ${wanted}`);
    await page.refresh(`[data-do="rename-${ix.id}"]`);
    return null;
  });
  save.querySelector(".form-actions").append(h("button", { type: "button", class: "quiet", onclick: back }, "Cancel"));
  save.addEventListener("keydown", (event) => {
    if (event.key === "Escape") back();
  });
  more.replaceChildren(save);
  input.input.focus();
  input.input.select();
}

// ---------- Adding one ----------

export async function indexerAdd(ctx) {
  const all = await api("/indexers");
  if (!ctx.current()) return [];
  const count = (n) => [n ? `${plural(n, "indexer", "indexers")} so far. ` : "No indexers yet. ", h("a", { href: HERE }, "See them"), "."];
  const added = h("p", { class: "note", role: "status" }, count(all.length));
  const adding = addIndexers(ctx, all, async () => {
    const fresh = await api("/indexers");
    adding.drawPresets(fresh);
    added.replaceChildren(...count(fresh.length));
    forgetHealth();
  });
  return [added, ...adding.groups];
}

// ---------- Soulseek ----------

// slskd, which searches Soulseek for albums and songs: how it is now, what it shares (which
// must be nothing), and its settings.
export async function soulseek(ctx) {
  const services = await api("/settings/services");
  if (!ctx.current()) return [];
  const slskd = services.find((s) => s.key === "slskd");
  if (!slskd) return [emptyState("This server has no Soulseek settings.")];
  const state = h("div", { role: "status" });
  const again = h("button", { type: "button", class: "quiet" }, "Ask again");
  const ask = async () => {
    again.disabled = true;
    state.replaceChildren(h("p", { class: "note" }, "Asking slskd…"));
    try {
      const said = await api("/settings/services/slskd/test", { method: "POST", body: { values: {} } });
      state.replaceChildren(h("p", {}, stateLabel(said.ok ? "have" : "failed", said.ok ? "Working" : "Not working")), h("p", {}, said.message));
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      state.replaceChildren(h("p", { class: "error" }, `Couldn't ask slskd: ${err.message}`));
    } finally {
      again.disabled = false;
    }
  };
  again.addEventListener("click", ask);
  if (slskd.on) ask();
  else state.replaceChildren(h("p", {}, stateLabel("off", "Off")), h("p", {}, "Soulseek isn't searched. Set slskd's address and an API key under Settings below to turn it on."));
  return [
    group(
      "How it is",
      h("p", { class: "note" }, "Albums are searched for on the indexers and on Soulseek at once, and the best accepted result wins. Songs wanted on their own come from Soulseek only. ", h("a", { href: "#/admin/music/songs" }, "Wanted songs"), "."),
      state,
      slskd.on ? h("div", { class: "form-actions" }, again) : null,
    ),
    group(
      "What it shares",
      h("p", {}, h("strong", {}, "It must share nothing.")),
      h(
        "p",
        { class: "note" },
        "No shared folder is mounted for slskd, Melete never changes what slskd shares, and Melete asks slskd what it shares before every search and grab: while it shares anything, Melete won't use it. If the answer above says it shares something, remove the shares in slskd's own settings.",
      ),
    ),
    group("Settings", serviceSection(ctx, slskd)),
  ];
}
