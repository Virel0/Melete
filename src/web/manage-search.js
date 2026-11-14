// Searching for one title from its Manage page: Search (ask every indexer and grab the best
// accepted result, as an RSS sync would) and Search and choose (show every result with its
// decision: how each indexer answered, the accepted ones with Grab, the rejected ones with why
// they can't be grabbed, and Blocklist on each). A past search's results show the same way.

import { when } from "/web/forms.js";
import { SignedOut, announce, api, h, number, plural, size } from "/web/lib.js";
import { stateLabel } from "/web/page-parts.js";
import { tryLink } from "/web/release-try.js";

// What the server says of a single episode a season's search found (`search::SINGLE_EPISODES`).
const SINGLE_EPISODES = "it's single episodes; this searched for a season";

// What a search for `what` asks: `path` and `body` for POST, `label` for the words.
// Search: the best accepted, not blocklisted result is grabbed; says what happened in `said`.
export async function searchAndGrab(ctx, ask, said) {
  said.classList.remove("error");
  said.textContent = `Searching every indexer for ${ask.label}…`;
  try {
    const search = await api(ask.path, { method: "POST", body: ask.body });
    const best = search.releases.find((r) => r.accepted && !r.blocklisted);
    // No pack of the season: its single episodes are taken for the missing ones, as a sync would.
    const single = (r) => !r.blocklisted && r.decision.rejections?.length === 1 && r.decision.rejections[0] === SINGLE_EPISODES;
    if (!best && search.releases.some(single)) {
      said.textContent = "No pack of the season was accepted; taking its single episodes for the missing ones…";
      const taken = await api(`/searches/${search.id}/take`, { method: "POST" });
      const started = taken.grabbed.filter((g) => g.download_id !== null).length;
      said.textContent = started
        ? `${outcomes(search).sentence} No pack of the season was accepted, so ${plural(started, "single episode was", "single episodes were")} grabbed for the missing episodes. Follow them under Activity.`
        : `${outcomes(search).sentence} No pack was accepted, and none of its single episodes could be taken${taken.grabbed[0]?.refused ? ` (${taken.grabbed[0].refused})` : " (every episode is on disk or on its way)"}. Search and choose shows why.`;
      announce(said.textContent);
      return search;
    }
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
            h("tbody", {}, search.releases.map((r) => releaseRow(ctx, r, names, search.profile_id))),
          ),
        )
      : h("p", { class: "status" }, "No indexer had anything for it."),
  );
}

function releaseRow(ctx, r, names, profileId) {
  const d = r.decision;
  const cell = (label, content, cls) => h("td", { "data-label": label, class: cls }, content);
  const reasons = d.rejections ?? d.reasons ?? [];
  // Single episodes of the season searched for, refused for nothing else: taken one by one for
  // the season's missing episodes when no pack of it is grabbed.
  const asEpisode = reasons.length === 1 && reasons[0] === SINGLE_EPISODES;
  const why = r.accepted
    ? null
    : asEpisode
      ? h("p", { class: "note" }, "Not a pack: taken for its episode when no pack of the season is grabbed and the episode is missing.")
      : h("p", { class: "why" }, `Can't be grabbed: ${reasons.join("; ") || "turned down"}. `, tryLink(r, profileId));
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
