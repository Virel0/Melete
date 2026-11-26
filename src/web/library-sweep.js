// Settings → Libraries: a movie or series library's Scan and search. The library is scanned,
// then, as ticked, its missing titles are searched for, then upgrades of its files below their
// cutoff, then the subtitles its files lack are fetched. Its latest is shown, and followed while
// it runs.

import { SignedOut, api, h } from "/web/lib.js";

const FOLLOW_MS = 5_000;

const STEPS = {
  scanning: "scanning the library",
  missing: "searching for what's missing",
  upgrades: "searching for upgrades",
  subtitles: "fetching missing subtitles",
};

// What a sweep says: under way, how it ended, or that there's been none.
function said(sweep) {
  if (!sweep) return "Not run yet.";
  if (sweep.state === "running") {
    const run = sweep.step === "missing" ? sweep.missing_search_id : sweep.step === "upgrades" ? sweep.upgrade_search_id : null;
    return `Under way: ${STEPS[sweep.step] ?? sweep.step}${run ? ` (search ${run}, followed under Activity)` : ""}…`;
  }
  return sweep.state === "failed" ? `Stopped: ${sweep.message}` : `Done: ${sweep.message}.`;
}

export function librarySweep(ctx, library) {
  const name = encodeURIComponent(library.name);
  const id = (what) => `sweep-${library.name.replace(/[^a-z0-9]+/gi, "-")}-${what}`;
  const check = (what, label) => {
    const input = h("input", { type: "checkbox", id: id(what), checked: true });
    return { input, el: h("div", { class: "check" }, input, h("label", { for: input.id }, label)) };
  };
  const missing = check("missing", "Search for what's missing");
  const upgrades = check("upgrades", "Search for upgrades of files below their cutoff");
  const subtitles = check("subtitles", "Fetch missing subtitles (the languages set for imports)");
  const status = h("p", { class: "note", role: "status" }, "Looking…");
  const button = h("button", { type: "button", class: "quiet" }, `Scan and search ${library.name}`);
  let following = null;

  const show = (sweep) => {
    status.textContent = `Scan and search: ${said(sweep)}`;
    button.disabled = sweep?.state === "running";
    clearTimeout(following);
    if (sweep?.state === "running") following = setTimeout(load, FOLLOW_MS);
  };
  async function load() {
    if (!ctx.current()) return;
    try {
      show(await api(`/libraries/${name}/scan-and-search`));
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      status.textContent = `Couldn't read its latest Scan and search: ${err.message}`;
    }
  }
  button.addEventListener("click", async () => {
    button.disabled = true;
    try {
      const body = { missing: missing.input.checked, upgrades: upgrades.input.checked, subtitles: subtitles.input.checked };
      show(await api(`/libraries/${name}/scan-and-search`, { method: "POST", body }));
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      status.textContent = err.message;
      button.disabled = false;
    }
  });
  load();
  return h(
    "details",
    { class: "sweep" },
    h("summary", {}, "Scan and search"),
    h(
      "p",
      { class: "note" },
      "Scans this library, then searches for its titles' missing episodes or films and for upgrades, one search at a time as Search all missing does, and grabs the best of each. Searches go to your indexers.",
    ),
    missing.el,
    upgrades.el,
    subtitles.el,
    h("div", { class: "form-actions" }, button),
    status,
  );
}
