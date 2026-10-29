// Searching for one title from its Manage page: Search (ask every indexer and grab the best
// accepted result, as an RSS sync would) and Search and choose (show every result with its
// decision: how each indexer answered, the accepted ones with Grab, the rejected ones with why
// they can't be grabbed, and Blocklist on each). A past search's results show the same way.

import { when } from "/web/forms.js";
import { SignedOut, announce, api, h, number, plural, size } from "/web/lib.js";
import { stateLabel } from "/web/page-parts.js";

// What a search for `what` asks: `path` and `body` for POST, `label` for the words.
// Search: the best accepted, not blocklisted result is grabbed; says what happened in `said`.
export async function searchAndGrab(ctx, ask, said) {
  said.classList.remove("error");
  said.textContent = `Searching every indexer for ${ask.label}…`;
  try {
    const search = await api(ask.path, { method: "POST", body: ask.body });
    const best = search.releases.find((r) => r.accepted && !r.blocklisted);
    if (!best) {
      said.textContent = `${outcomes(search).sentence} Nothing was accepted, so nothing was grabbed. Search and choose shows why.`;
      announce(said.textContent);
      return search;
    }
    said.textContent = `Grabbing ${best.title}…`;
    await api(`/releases/${best.id}/grab`, { method: "POST" });
    said.textContent = `Grabbed ${best.title} (${best.decision.quality}${best.size ? `, ${size(best.size)}` : ""}). Follow it under Activity.`;
    announce(said.textContent);
    return search;
  } catch (err) {
    if (err instanceof SignedOut) return ctx.signedOut();
    said.textContent = err.message;
    said.classList.add("error");
    return null;
  }
}

// How the indexers answered: a sentence, and each one's outcome in a line of its own.
function outcomes(search) {
  const answered = search.indexers.filter((i) => !i.error);
  const failedOnes = search.indexers.filter((i) => i.error);
  const sentence = search.indexers.length
    ? `${plural(search.indexers.length, "indexer", "indexers")} asked: ${number.format(answered.length)} answered with ${plural(search.releases.length, "result", "results")}${failedOnes.length ? `, ${number.format(failedOnes.length)} failed` : ""}.`
    : "No indexer was asked.";
  const each = [
    ...search.indexers.map((i) => (i.error ? stateLabel("failed", `${i.name}: ${i.error}`) : stateLabel("have", `${i.name}: ${plural(i.results, "result", "results")}`))),
    search.soulseek ? (search.soulseek.error ? stateLabel("failed", `Soulseek: ${search.soulseek.error}`) : stateLabel("have", `Soulseek: ${plural(search.soulseek.results, "result", "results")}`)) : null,
  ].filter(Boolean);
  return { sentence, each };
}

// Search and choose: runs the search and shows its results in `box`.
export async function searchAndChoose(ctx, ask, box) {
  box.replaceChildren(h("p", { class: "status", role: "status" }, `Searching every indexer for ${ask.label}…`));
  try {
    const search = await api(ask.path, { method: "POST", body: ask.body });
    if (!ctx.current()) return;
    box.replaceChildren(results(ctx, search, ask.label));
    box.querySelector("h3")?.focus();
  } catch (err) {
    if (err instanceof SignedOut) return ctx.signedOut();
    box.replaceChildren(h("p", { class: "error", role: "alert" }, `Couldn't search for ${ask.label}: ${err.message}`));
  }
}

// A search's results: how each indexer answered, then every release, accepted ones first.
export function results(ctx, search, label) {
  const names = new Map(search.indexers.map((i) => [i.indexer_id, i.name]));
  const { sentence, each } = outcomes(search);
  const accepted = search.releases.filter((r) => r.accepted).length;
  return h(
    "div",
    { class: "choose" },
    h("h3", { tabindex: "-1" }, `Results for ${label}`),
    h("p", {}, `${sentence} ${number.format(accepted)} accepted. Searched ${when(search.searched_at)}.`),
    h("ul", { class: "outcomes" }, each.map((x) => h("li", {}, x))),
    search.releases.length
      ? h(
          "div",
          { class: "table-wrap stacks" },
          h(
            "table",
            { class: "data releases" },
            h("caption", { class: "visually-hidden" }, `Results for ${label}, accepted first`),
            h("thead", {}, h("tr", {}, ["Release", "Quality", "Size", "Seeders", "Score", "Indexer", "Action"].map((c) => h("th", { scope: "col", class: ["Size", "Seeders", "Score"].includes(c) ? "r" : null }, c)))),
            h("tbody", {}, search.releases.map((r) => releaseRow(ctx, r, names))),
          ),
        )
      : h("p", { class: "status" }, "No indexer had anything for it."),
  );
}

function releaseRow(ctx, r, names) {
  const d = r.decision;
  const cell = (label, content, cls) => h("td", { "data-label": label, class: cls }, content);
  const why = r.accepted ? null : h("p", { class: "why" }, `Can't be grabbed: ${(d.rejections ?? d.reasons ?? []).join("; ") || "turned down"}.`);
  const formats = d.formats?.length ? h("p", { class: "note" }, `Formats: ${d.formats.map((f) => `${f.name} ${f.score > 0 ? "+" : ""}${f.score}`).join(", ")}.`) : null;
  const actions = releaseActions(ctx, r);
  return h(
    "tr",
    { class: r.accepted ? "accepted" : null },
    h("th", { scope: "row" }, h("span", { class: "path" }, r.title), r.source === "soulseek" ? [" ", h("span", { class: "badge" }, "Soulseek")] : null, why, formats),
    cell("Quality", d.quality ?? "—"),
    cell("Size", r.size ? size(r.size) : "—", "r"),
    cell("Seeders", r.seeders ?? "—", "r"),
    cell("Score", d.score === undefined ? "—" : `${d.score > 0 ? "+" : ""}${d.score}`, "r"),
    cell("Indexer", r.source === "soulseek" ? "Soulseek" : names.get(r.indexer_id) ?? "—"),
    cell("Action", actions),
  );
}

// Grab on an accepted result ("Not accepted" on the rest), and Blocklist on any not on it.
function releaseActions(ctx, r) {
  const said = h("span", { class: "note", role: "status" });
  const blocked = stateLabel("failed", "Blocklisted");
  const grab = r.accepted && !r.blocklisted ? h("button", { type: "button", "aria-label": `Grab ${r.title}` }, "Grab") : null;
  grab?.addEventListener("click", async () => {
    grab.disabled = true;
    said.textContent = r.source === "soulseek" ? "Asking slskd for it…" : "Sending to Deluge…";
    try {
      await api(`/releases/${r.id}/grab`, { method: "POST" });
      said.textContent = "Grabbed. Follow it under Activity.";
      announce(`Grabbed ${r.title}`);
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      said.textContent = err.message;
      grab.disabled = false;
    }
  });
  // Never grabbed again, by hand or by a sync; undone under Indexers, Blocklist.
  const block = r.blocklisted ? null : h("button", { type: "button", class: "quiet", "aria-label": `Blocklist ${r.title}` }, "Blocklist");
  block?.addEventListener("click", async () => {
    block.disabled = true;
    try {
      await api("/blocklist", { method: "POST", body: { release_id: r.id } });
      grab?.remove();
      block.replaceWith(blocked);
      said.textContent = "It's never grabbed now. Take it off under Indexers, Blocklist.";
      announce(`Blocklisted ${r.title}`);
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      said.textContent = err.message;
      block.disabled = false;
    }
  });
  const first = r.blocklisted ? blocked : grab ?? h("span", { class: "note" }, "Not accepted");
  return h("div", { class: "row-actions" }, first, block, said);
}
