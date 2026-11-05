// Make room: the service's titles suggested for freeing space, the strongest first, each with
// what to do with it (delete it, swap it for a smaller release, or compress it), the space that
// frees and why, and ticks to choose several with the space they'd free together. Deleting
// moves a title's files to its library's trash, where they wait 30 days and can be put back
// (Replaced files). Swapping searches, slowly, for a release at most 1080p and clearly smaller,
// which replaces the copy once it's in. Compressing converts its H.264 files to HEVC on the GPU,
// one at a time, each copy replacing its file once checked. What's under way is listed above
// the suggestions.

import { confirmInline } from "/web/forms.js";
import { SignedOut, announce, api, h, plural, size } from "/web/lib.js";
import { emptyState, failed, placeholders } from "/web/page-parts.js";

const ACTIONS = { delete: "Delete", swap: "Swap for a smaller release", compress: "Compress" };
const REASONS = {
  never_watched: "Never watched",
  not_watched_lately: "Not watched lately",
  untouched: "Untouched for a year",
  low_rating: "Low rating",
  large_copy: "Large copy",
  compressible: "Compresses well",
};

// Which suggestions belong to the service: anime in Anime, the rest by kind.
function ofService(key) {
  if (key === "anime") return (s) => s.anime;
  const kind = key === "movies" ? "movie" : "series";
  return (s) => !s.anime && s.kind === kind;
}

export async function makeRoomFor(ctx) {
  const box = h("div", {}, placeholders("suggestions", { kind: "rows" }));
  load(ctx, box);
  return [
    h(
      "p",
      { class: "lead" },
      "Titles to free space with, the most worth acting on first. Nothing anyone is watching, has on their list or asked for lately is listed. Deleting moves a title's files to its library's trash for 30 days, where Replaced files can put them back.",
    ),
    box,
  ];
}

async function load(ctx, box) {
  try {
    const found = await api("/admin/make-room");
    if (!ctx.current()) return;
    const mine = ofService(ctx.service.key);
    box.replaceChildren(underWay(found.swapping.filter(mine), found.downloads), compressing(found.compressing.filter(mine)), ...view(ctx, found.items.filter(mine)));
  } catch (err) {
    if (err instanceof SignedOut) return ctx.signedOut();
    box.replaceChildren(failed("the suggestions", err, () => load(ctx, box)));
  }
}

function view(ctx, items) {
  if (!items.length) {
    return [emptyState(`${ctx.service.name} has nothing worth suggesting now.`, null)];
  }
  const total = items.reduce((sum, s) => sum + s.saves, 0);
  const chosen = new Set();
  const said = h("p", { class: "status", role: "status" });
  const bulk = h("div", { class: "row-actions" });
  const deletable = items.filter((s) => s.action === "delete");
  const redraw = () => {
    const picked = deletable.filter((s) => chosen.has(key(s)));
    const frees = picked.reduce((sum, s) => sum + s.saves, 0);
    said.textContent = picked.length ? `${plural(picked.length, "title", "titles")} chosen, freeing ${size(frees)}.` : "";
    bulk.replaceChildren(
      picked.length
        ? h("button", { type: "button", class: "danger", onclick: () => confirmMany(ctx, bulk, picked, frees) }, `Move ${plural(picked.length, "title", "titles")} to the trash`)
        : "",
    );
  };
  const rows = items.map((s) => row(ctx, s, chosen, redraw));
  redraw();
  return [
    h("p", {}, `${plural(items.length, "suggestion", "suggestions")}, freeing ${size(total)} in all.`),
    h(
      "div",
      { class: "table-wrap stacks" },
      h(
        "table",
        { class: "data make-room" },
        h("caption", { class: "visually-hidden" }, "Titles suggested for making room"),
        h("thead", {}, h("tr", {}, ["Choose", "Title", "Suggested", "Frees", "Why", ""].map((c) => h("th", { scope: "col", class: c === "Frees" ? "r" : null }, c)))),
        h("tbody", {}, rows),
      ),
    ),
    said,
    bulk,
  ];
}

const key = (s) => `${s.kind}:${s.id}`;

function row(ctx, s, chosen, redraw) {
  const label = `${s.title}${s.year ? ` (${s.year})` : ""}`;
  const pick =
    s.action === "delete"
      ? h("input", { type: "checkbox", "aria-label": `Choose ${label}` })
      : null;
  pick?.addEventListener("change", () => {
    if (pick.checked) chosen.add(key(s));
    else chosen.delete(key(s));
    redraw();
  });
  const actions = h("div", { class: "row-actions" });
  if (s.action === "delete") {
    actions.append(h("button", { type: "button", class: "quiet", "aria-label": `Move ${label} to the trash`, onclick: () => confirmOne(ctx, actions, s, label) }, "Move to trash"));
  } else if (s.action === "compress") {
    actions.append(h("button", { type: "button", class: "quiet", "aria-label": `Compress ${label}`, onclick: () => confirmCompress(ctx, actions, s, label) }, "Compress"));
  } else if (s.action === "swap") {
    actions.append(h("button", { type: "button", class: "quiet", "aria-label": `Swap ${label} for a smaller release`, onclick: () => confirmSwap(ctx, actions, s, label) }, "Swap"));
  } else {
    actions.append(h("span", { class: "note" }, "Not available yet"));
  }
  const cell = (name, content, cls) => h("td", { "data-label": name, class: cls }, content);
  return h(
    "tr",
    {},
    h("td", { class: "tick" }, pick ?? ""),
    h("th", { scope: "row" }, h("a", { href: `#/admin/${ctx.service.key}/title/f${s.id}` }, label)),
    cell("Suggested", ACTIONS[s.action] ?? s.action),
    cell("Frees", size(s.saves), "r"),
    cell("Why", s.reasons.map((r) => REASONS[r] ?? r).join(", ") || "—"),
    cell("", actions),
  );
}

// Swaps under way, by title: what's left to search, on its way, done, and not found. While
// downloads are off, nothing is searched, and it says so.
function underWay(swapping, downloads) {
  if (!swapping.length) return "";
  const waiting = swapping.some((s) => s.waiting > 0);
  const line = (s) => {
    const parts = [
      s.waiting && `${number(s.waiting)} waiting`,
      s.grabbed && `${number(s.grabbed)} on the way`,
      s.done && `${number(s.done)} done`,
      s.none_found && `${number(s.none_found)} with nothing smaller found`,
      s.failed && `${number(s.failed)} failed`,
    ].filter(Boolean);
    return h("li", {}, h("strong", {}, s.title), `: ${parts.join(", ")}.`);
  };
  const off = !downloads && waiting ? h("p", { class: "note" }, "Downloads are off, so swaps wait. Turn them on in System → Outside services → Deluge.") : "";
  return h("section", { class: "swaps", "aria-label": "Swaps under way" }, h("h2", {}, "Swaps"), off, h("ul", {}, swapping.map(line)));
}

const number = (n) => new Intl.NumberFormat().format(n);

// Compressions under way, by title: waiting, being converted, done (and what that freed), failed.
function compressing(list) {
  if (!list.length) return "";
  const line = (c) => {
    const parts = [
      c.waiting && `${number(c.waiting)} waiting`,
      c.running && "converting now",
      c.done && `${number(c.done)} done, freeing ${size(c.saved)}`,
      c.failed && `${number(c.failed)} failed`,
    ].filter(Boolean);
    return h("li", {}, h("strong", {}, c.title), `: ${parts.join(", ")}.`);
  };
  return h("section", { class: "swaps", "aria-label": "Compressions under way" }, h("h2", {}, "Compressing"), h("ul", {}, list.map(line)));
}

function confirmCompress(ctx, actions, s, label) {
  confirmInline(actions, {
    question: `Convert ${label}'s H.264 files to HEVC, one at a time while nothing else is converted, each copy replacing its file once it plays as long with the same tracks (the old file goes to the trash for 30 days)?`,
    confirm: "Compress",
    run: async () => {
      const done = await api("/admin/make-room/compress", { method: "POST", body: { kind: s.kind, id: s.id } });
      const words = `${label}: ${plural(done.files, "file", "files")} queued to compress.`;
      announce(words);
      ctx.reload(words);
    },
  });
}

function confirmSwap(ctx, actions, s, label) {
  confirmInline(actions, {
    question: `Look for a smaller release of ${label}, at most 1080p, and replace its copy once it's in (the old one goes to the trash for 30 days)? It stops being upgraded.${s.kind === "series" ? " Each episode is searched on its own, a few a minute." : ""}`,
    confirm: "Swap",
    run: async () => {
      const done = await api("/admin/make-room/swap", { method: "POST", body: { kind: s.kind, id: s.id } });
      const words = `${label}: ${plural(done.files, "file", "files")} queued to swap for a smaller release.`;
      announce(words);
      ctx.reload(words);
    },
  });
}

async function remove(s) {
  return api("/admin/make-room/delete", { method: "POST", body: { kind: s.kind, id: s.id } });
}

function confirmOne(ctx, actions, s, label) {
  confirmInline(actions, {
    question: `Move all of ${label}'s files (${size(s.size)}) to the trash? They're deleted in 30 days unless put back.`,
    confirm: "Move to trash",
    run: async () => {
      const done = await remove(s);
      announce(`${label}: ${plural(done.files, "file", "files")} moved to the trash.`);
      ctx.reload(`${label} moved to the trash.`);
    },
  });
}

function confirmMany(ctx, bulk, picked, frees) {
  confirmInline(bulk, {
    question: `Move all the files of ${plural(picked.length, "title", "titles")} (${size(frees)}) to the trash? They're deleted in 30 days unless put back.`,
    confirm: `Move ${plural(picked.length, "title", "titles")} to the trash`,
    run: async () => {
      const failures = [];
      for (const s of picked) {
        try {
          await remove(s);
        } catch (err) {
          if (err instanceof SignedOut) throw err;
          failures.push(`${s.title}: ${err.message}`);
        }
      }
      const moved = picked.length - failures.length;
      const words = `${plural(moved, "title", "titles")} moved to the trash.${failures.length ? ` Not moved: ${failures.join("; ")}` : ""}`;
      announce(words);
      ctx.reload(words);
    },
  });
}
