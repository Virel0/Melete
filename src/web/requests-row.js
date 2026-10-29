// A request as it's listed: what it is, where it's got to season by season, taking it back, and
// changing the season a waiting one asks for.

import { confirmInline, when } from "/web/forms.js";
import { SignedOut, api, h, metaRow, plural, seasonName } from "/web/lib.js";
import { stepBar } from "/web/progress.js";

export const KINDS = { movie: "Movie", series: "Series", artist: "Artist", album: "Album", song: "Song" };
const STATES = { requested: "Waiting for an admin", approved: "Approved", declined: "Declined" };

// What it is, in a few words: its year, who it's by, which season.
export function facts(r) {
  return metaRow([
    KINDS[r.kind],
    r.year ? String(r.year) : null,
    r.artist ? `by ${r.artist}` : null,
    r.kind === "series" ? (r.season === null || r.season === undefined ? "Whole series" : seasonName(r.season)) : null,
  ]);
}

// Where a season stands, in the words the step bar uses.
const SEASON_STEPS = {
  searching: "Looking for a download",
  downloading: "Downloading",
  transferring: "Transferring",
  importing: "Importing",
  in_library: "In your library",
  failed: "Failed",
  wanted: "Not asked for",
};

// "3 of 10 episodes", or as much as is known.
function episodeCount(s) {
  if (s.episodes === null) return s.in_library ? `${plural(s.in_library, "episode", "episodes")} here` : null;
  if (s.episodes === 0) return s.in_library ? `${plural(s.in_library, "episode", "episodes")} here` : "Not aired yet";
  return `${s.in_library} of ${plural(s.episodes, "episode", "episodes")}`;
}

function seasonComplete(s) {
  return s.episodes !== null && s.episodes > 0 && s.in_library >= s.episodes;
}

// One season: its name, where it stands, how much is here, and asking for it when nobody has.
function seasonItem(ctx, r, s, waiting) {
  const label =
    s.step === "downloading" && s.percent !== null
      ? `Downloading, ${Math.round(s.percent)}%`
      : s.step === "searching" && s.episodes === 0
        ? "Waiting for it to air"
        : (SEASON_STEPS[s.step] ?? s.step);
  const actions = h("span", { class: "season-actions" });
  if (waiting) {
    actions.append(h("span", { class: "note" }, "You asked; waiting for an admin."));
  } else if (!s.monitored && !seasonComplete(s)) {
    const said = h("span", { class: "note", role: "status" });
    const button = h("button", { type: "button", class: "quiet" }, `Ask for ${seasonName(s.season).toLowerCase()}`);
    button.addEventListener("click", async () => {
      button.disabled = true;
      said.textContent = "Asking…";
      try {
        const asked = await api("/requests", { method: "POST", body: { kind: "series", tmdb_id: r.tmdb_id, season: s.season } });
        const message =
          asked.state === "approved"
            ? `Asked for ${seasonName(s.season).toLowerCase()} of ${r.title}; it's being looked for.`
            : `Asked for ${seasonName(s.season).toLowerCase()} of ${r.title}. An admin will decide.`;
        ctx.reload(message);
      } catch (err) {
        if (err instanceof SignedOut) return ctx.signedOut();
        said.textContent = err.message;
        button.disabled = false;
      }
    });
    actions.append(button, said);
  }
  return h(
    "li",
    {},
    h("span", { class: "season-name" }, seasonName(s.season)),
    h("span", { class: `badge${s.step === "failed" ? " state-failed" : ""}` }, label),
    s.step === "downloading" && s.percent !== null
      ? h("progress", { max: 100, value: s.percent, "aria-label": `${seasonName(s.season)}: ${Math.round(s.percent)}% downloaded` })
      : null,
    episodeCount(s) ? h("span", { class: "note" }, episodeCount(s)) : null,
    actions,
  );
}

// A series request's seasons: the ones it asked for, then (on the newest request for the
// series) the others, which can be asked for from here.
function seasonList(ctx, r, { items = [], others = false, open } = {}) {
  const waiting = (season) =>
    items.some((o) => o.kind === "series" && o.tmdb_id === r.tmdb_id && o.season === season && o.state === "requested");
  const own = r.season === null ? r.seasons : r.seasons.filter((s) => s.season === r.season);
  const list = h("ul", { class: "request-seasons", "aria-label": `Seasons of ${r.title}` }, own.map((s) => seasonItem(ctx, r, s, false)));
  if (r.season === null || !others) return list;
  // Seasons asked for in another of your requests have their own row.
  const mine = new Set(items.filter((o) => o.kind === "series" && o.tmdb_id === r.tmdb_id && o.state === "approved").map((o) => o.season));
  const rest = mine.has(null) ? [] : r.seasons.filter((s) => s.season !== r.season && !mine.has(s.season));
  if (!rest.length) return list;
  const askable = rest.filter((s) => !s.monitored && !seasonComplete(s) && !waiting(s.season)).length;
  const more = h(
    "details",
    { open: open?.has(r.id) },
    h("summary", {}, askable ? `Other seasons (${askable} you can ask for)` : "Other seasons"),
    h("ul", { class: "request-seasons" }, rest.map((s) => seasonItem(ctx, r, s, waiting(s.season)))),
  );
  more.addEventListener("toggle", () => (more.open ? open?.add(r.id) : open?.delete(r.id)));
  return [list, more];
}

// A request as its requester sees it: where it's got to, and taking it back while it waits.
export function requestRow(ctx, r, { decide: deciding, ...mineOpts } = {}) {
  const actions = h("div", { class: "row-actions" });
  if (r.state === "requested" && r.kind === "series") {
    actions.append(h("button", { type: "button", class: "quiet", onclick: () => changingSeason(ctx, r, actions) }, "Change season"));
  }
  if (r.state === "requested" && !deciding) {
    const back = h("button", { type: "button", class: "quiet" }, "Take back");
    back.addEventListener("click", () =>
      confirmInline(actions, {
        question: `Take back your request for ${r.title}?`,
        confirm: "Take back request",
        keep: "Keep it",
        run: async () => {
          await api(`/requests/${r.id}`, { method: "DELETE" });
          ctx.reload(`Took back your request for ${r.title}.`);
        },
      }),
    );
    actions.append(back);
  }
  const state = h("span", { class: `badge${r.state === "declined" ? " state-failed" : ""}` }, STATES[r.state] ?? r.state);
  // A series goes season by season; one step for all of it would say "in your library" once
  // any episode is.
  const progress =
    r.seasons?.length && !deciding
      ? seasonList(ctx, r, mineOpts)
      : r.state === "approved" && r.progress
        ? stepBar(r.progress)
        : r.state === "approved" && r.kind === "artist"
          ? h("p", { class: "note" }, "Their albums are looked for one by one.")
          : null;
  return h(
    "li",
    {},
    h(
      "div",
      {},
      h("strong", {}, r.title),
      " ",
      state,
      facts(r),
      h("p", { class: "note" }, deciding ? `Asked for by ${r.username}, ${when(r.created_at)}.` : `Asked for ${when(r.created_at)}.`),
      r.state === "declined" && r.reason ? h("p", { class: "note" }, `Why: ${r.reason}`) : null,
      progress,
    ),
    actions,
  );
}

// Swaps a waiting series request's actions for the season it asks for, to change it.
function changingSeason(ctx, r, actions) {
  const before = [...actions.childNodes];
  const id = `season-change-${r.id}`;
  const season = h("input", { id, inputmode: "numeric", autocomplete: "off", class: "year", value: r.season ?? "" });
  const said = h("span", { class: "note", role: "status", id: `${id}-said` });
  const save = h("button", { type: "button" }, "Save season");
  const cancel = h("button", { type: "button", class: "quiet" }, "Cancel");
  const back = () => {
    actions.replaceChildren(...before);
    before.find((n) => n.tagName === "BUTTON")?.focus();
  };
  cancel.addEventListener("click", back);
  const go = async () => {
    const wanted = season.value.trim();
    if (wanted && !/^\d+$/.test(wanted)) {
      said.textContent = "Enter a season number, or leave it empty for the whole series.";
      season.setAttribute("aria-invalid", "true");
      season.setAttribute("aria-describedby", said.id);
      season.focus();
      return;
    }
    const chosen = wanted ? Number(wanted) : null;
    if (chosen === r.season) return back();
    save.disabled = cancel.disabled = true;
    try {
      await api(`/requests/${r.id}`, { method: "PUT", body: { season: chosen } });
      ctx.reload(`${r.title} now asks for ${chosen === null ? "the whole series" : seasonName(chosen).toLowerCase()}.`);
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      said.textContent = err.message;
      save.disabled = cancel.disabled = false;
    }
  };
  save.addEventListener("click", go);
  season.addEventListener("keydown", (event) => {
    if (event.key === "Enter") go();
    if (event.key === "Escape") back();
  });
  actions.replaceChildren(h("label", { for: id, class: "question" }, "Season (empty for the whole series)"), season, save, cancel, said);
  season.focus();
  season.select();
}
