// "Search all missing" for wanted movies, series and albums: starting a run, following it while
// it runs, and stopping it.

import { when } from "/web/forms.js";
import { SignedOut, announce, api, h, number, plural } from "/web/lib.js";

const FOLLOW_MS = 3000;
const MISSING = {
  movies: { what: "every monitored movie that isn't in a library or on its way", noun: "movies" },
  series: { what: "every monitored season still missing episodes, with nothing on its way", noun: "seasons" },
  albums: { what: "every monitored album of a monitored artist that isn't on disk or downloading", noun: "albums" },
};

// "Message" as a sentence: a capital first, a full stop last.
const sentence = (text) => (text ? `${text[0].toUpperCase()}${text.slice(1)}${/[.!?]$/.test(text) ? "" : "."}` : "");

// A run still going, or stopping after the search under way.
const under = (r) => r?.state === "running" || r?.state === "stopping";

// What's said about a run: how far it has got while it runs, or how it ended, and why some
// weren't searched for or grabbed.
function runLines(run, about) {
  const lines = [];
  if (under(run)) {
    const finding = run.total === 0 && run.done === 0;
    const now =
      run.state === "stopping"
        ? "Stopping after the search under way…"
        : finding
          ? `Working out which ${about.noun} are missing…`
          : run.current
            ? `Searching for ${run.current} (${number.format(run.done + 1)} of ${number.format(run.total)}).`
            : `Waiting before the next search (${number.format(run.done)} of ${number.format(run.total)} done).`;
    lines.push(
      h("p", {}, now),
      finding ? null : h("progress", { max: Math.max(run.total, 1), value: run.done, "aria-label": `${number.format(run.done)} of ${plural(run.total, about.noun.slice(0, -1), about.noun)} dealt with` }),
      h("p", { class: "note" }, `${plural(run.searched, "search", "searches")} made, ${number.format(run.grabbed)} grabbed so far. Started ${when(run.started_at)}${run.started_by ? ` by ${run.started_by}` : ""}.`),
    );
  } else if (run) {
    const how = { done: "finished", stopped: "stopped", failed: "failed" }[run.state] ?? run.state;
    lines.push(h("p", {}, `Last run ${how} ${when(run.finished_at)}${run.started_by ? ` (started by ${run.started_by})` : ""}. ${sentence(run.message ?? "")}`));
  }
  if (run?.notes.length) {
    lines.push(
      h(
        "details",
        {},
        h("summary", {}, `Why some weren't searched for or grabbed (${number.format(run.notes.length)})`),
        h("ul", { class: "notes" }, run.notes.map((n) => h("li", {}, n))),
      ),
    );
  }
  return lines.filter(Boolean);
}

// "Search all missing": one run per area in the background, followed here while it runs
// and stoppable. The server paces it; this only starts, shows and stops it. `latest` is the
// area's latest run from GET /missing-searches, or null.
export function searchMissing(ctx, area, latest) {
  const about = MISSING[area];
  const said = h("div", { role: "status" });
  const start = h("button", { type: "button" }, "Search all missing");
  const stop = h("button", { type: "button", class: "quiet" }, "Stop searching");
  let run = latest;
  let timer = null;

  const draw = () => {
    said.replaceChildren(...runLines(run, about));
    start.hidden = under(run);
    stop.hidden = !under(run);
    stop.disabled = run?.state === "stopping";
  };

  const follow = () => {
    clearInterval(timer);
    const look = async () => {
      // Drawn again (a reload), this copy is no longer on the page.
      if (!said.isConnected) return clearInterval(timer);
      try {
        run = await api(`/missing-searches/${run.id}`);
        draw();
        if (!under(run)) {
          clearInterval(timer);
          announce(`Search all missing ${{ done: "finished", stopped: "stopped", failed: "failed" }[run.state] ?? run.state}. ${sentence(run.message ?? "")}`);
          if (document.activeElement === stop || document.activeElement === document.body) start.focus();
        }
      } catch (err) {
        clearInterval(timer);
        if (err instanceof SignedOut) return ctx.signedOut();
        said.replaceChildren(h("p", { class: "error" }, `Couldn't follow the search: ${err.message}. It goes on regardless; reload the page to see how it's getting on.`));
      }
    };
    timer = setInterval(look, FOLLOW_MS);
  };
  ctx.onLeave(() => clearInterval(timer));

  start.addEventListener("click", async () => {
    start.disabled = true;
    try {
      run = await api("/missing-searches", { method: "POST", body: { area } });
      draw();
      announce(`Searching for every missing one of the ${about.noun}`);
      stop.focus();
      follow();
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      said.replaceChildren(h("p", { class: "error" }, err.message));
    } finally {
      start.disabled = false;
    }
  });
  stop.addEventListener("click", async () => {
    stop.disabled = true;
    try {
      run = await api(`/missing-searches/${run.id}/stop`, { method: "POST" });
      draw();
      announce("Stopping after the search under way");
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      said.replaceChildren(h("p", { class: "error" }, err.message));
      stop.disabled = false;
    }
  });

  draw();
  if (under(run)) follow();
  return h(
    "div",
    { class: "missing-search" },
    h("p", { class: "note" }, `Searches for ${about.what}, one at a time and at most four a minute, so no indexer is asked too often; it stops if an indexer says it has reached its limit. When downloads are on, the best result of each is grabbed.`),
    said,
    h("div", { class: "row-actions" }, start, stop),
  );
}
