// Make room: the service's titles suggested for freeing space, the strongest first, each with
// what to do with it (delete it, swap it for a smaller release, or compress it), the space that
// frees and why, and ticks to choose several with the space they'd free together. Deleting
// moves a title's files to its library's trash, where they wait 30 days and can be put back
// (Replaced files); swapping and compressing come later, and say so.

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
    box.replaceChildren(...view(ctx, found.items.filter(ofService(ctx.service.key))));
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
