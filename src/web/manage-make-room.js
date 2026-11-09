// Make room: the service's titles suggested for freeing space, the strongest first, each with
// what's suggested (delete it, swap it for a smaller release, or compress it), the space that
// frees and why, and the other actions it can take instead. Ticked titles are acted on together
// from a bar that stays at the foot of the screen; the columns sort. Acting on a title draws the
// list again where it is, keeping the sort, what's ticked and the place on the page. Deleting
// moves a title's files to its library's trash, where they wait 30 days and can be put back
// (Replaced files). Swapping searches, slowly, for a release at most 1080p and clearly smaller,
// which replaces the copy once it's in. Compressing converts its H.264 files to HEVC on the GPU,
// one at a time, each copy replacing its file once checked. What's under way is listed above
// the suggestions.

import { confirmInline } from "/web/forms.js";
import { SignedOut, api, h, plural, size } from "/web/lib.js";
import { emptyState, failed, placeholders } from "/web/page-parts.js";

const ACTIONS = { delete: "Delete", swap: "Swap for a smaller release", compress: "Compress" };
// What a row's button for each action says, and the route that does it.
const BUTTONS = { delete: "Move to trash", swap: "Swap", compress: "Compress" };
const ROUTES = { delete: "/admin/make-room/delete", swap: "/admin/make-room/swap", compress: "/admin/make-room/compress" };
const REASONS = {
  never_watched: "Never watched",
  not_watched_lately: "Not watched lately",
  untouched: "Untouched for a year",
  low_rating: "Low rating",
  large_copy: "Large copy",
  compressible: "Compresses well",
};
// The orders the list can be in: the server's (most worth acting on first), or by a column.
// Each column's first click sorts the way it's most often wanted: sizes largest first.
const SORTS = {
  score: { name: "Most worth acting on first" },
  title: { name: "Title", by: (a, b) => a.title.localeCompare(b.title), first: "ascending" },
  action: { name: "Suggested", by: (a, b) => ACTIONS[a.action].localeCompare(ACTIONS[b.action]), first: "ascending" },
  size: { name: "Size", by: (a, b) => a.size - b.size, first: "descending" },
  saves: { name: "Frees", by: (a, b) => a.saves - b.saves, first: "descending" },
};

// Which suggestions belong to the service: anime in Anime, the rest by kind.
function ofService(key) {
  if (key === "anime") return (s) => s.anime;
  const kind = key === "movies" ? "movie" : "series";
  return (s) => !s.anime && s.kind === kind;
}

const key = (s) => `${s.kind}:${s.id}`;
const labelOf = (s) => `${s.title}${s.year ? ` (${s.year})` : ""}`;
const optionOf = (s, action) => s.options.find((o) => o.action === action);

export async function makeRoomFor(ctx) {
  // What stays as the list is drawn again: the order, what's ticked, and the bar's words.
  const v = { ctx, sort: "score", direction: "descending", chosen: new Set(), items: [] };
  v.box = h("div", {}, placeholders("suggestions", { kind: "rows" }));
  v.said = h("p", { class: "form-result", role: "status" });
  v.bar = h("div", { class: "select-bar", role: "region", "aria-label": "Chosen titles", hidden: true });
  load(v);
  return [
    h(
      "p",
      { class: "lead" },
      "Titles to free space with, the most worth acting on first. Nothing anyone is watching, has on their list or asked for lately is listed. Deleting moves a title's files to its library's trash for 30 days, where Replaced files can put them back.",
    ),
    v.box,
    v.bar,
  ];
}

// Fetches the suggestions and draws them in place of what's there; `said` goes in the bar.
async function load(v, said) {
  const { ctx } = v;
  try {
    const found = await api("/admin/make-room");
    if (!ctx.current()) return;
    const mine = ofService(ctx.service.key);
    v.items = found.items.filter(mine);
    // A title acted on may have left the list; it's no longer ticked.
    const here = new Set(v.items.map(key));
    for (const k of [...v.chosen]) if (!here.has(k)) v.chosen.delete(k);
    v.list = h("div", {}, view(v));
    v.box.replaceChildren(underWay(found.swapping.filter(mine), found.downloads), compressing(found.compressing.filter(mine)), v.list);
    v.said.textContent = said ?? "";
    drawBar(v);
  } catch (err) {
    if (err instanceof SignedOut) return ctx.signedOut();
    v.box.replaceChildren(failed("the suggestions", err, () => load(v)));
  }
}

// After acting: the list drawn again where it is, the result said in the bar, and focus there,
// as the button pressed may be gone.
async function acted(v, words) {
  await load(v, words);
  if (!v.bar.hidden) v.said.focus();
}

function sorted(v) {
  const sort = SORTS[v.sort];
  if (!sort.by) return v.items;
  const sign = v.direction === "ascending" ? 1 : -1;
  return [...v.items].sort((a, b) => sign * sort.by(a, b) || a.title.localeCompare(b.title));
}

function view(v) {
  const { ctx, items } = v;
  if (!items.length) {
    return [emptyState(`${ctx.service.name} has nothing worth suggesting now.`, null)];
  }
  const total = items.reduce((sum, s) => sum + s.saves, 0);
  const rows = sorted(v).map((s) => row(v, s));
  const boxes = () => [...v.list.querySelectorAll("tbody .tick input")];
  const everything = h("input", { type: "checkbox", id: "make-room-all" });
  const tickedAll = () => {
    everything.checked = v.chosen.size === items.length;
    everything.indeterminate = v.chosen.size > 0 && v.chosen.size < items.length;
  };
  tickedAll();
  everything.addEventListener("change", () => {
    for (const s of items) everything.checked ? v.chosen.add(key(s)) : v.chosen.delete(key(s));
    for (const box of boxes()) box.checked = everything.checked;
    drawBar(v);
  });
  v.tickedAll = tickedAll;
  const header = (name, sortKey, cls) => {
    if (!sortKey) return h("th", { scope: "col", class: cls }, name);
    const on = v.sort === sortKey;
    const button = h("button", { type: "button", class: "sort" }, name, h("span", { "aria-hidden": "true" }, on ? (v.direction === "ascending" ? " ↑" : " ↓") : ""));
    button.addEventListener("click", () => {
      const first = SORTS[sortKey].first;
      v.direction = on ? (v.direction === "ascending" ? "descending" : "ascending") : first;
      v.sort = sortKey;
      redraw(v, `sort-${sortKey}`);
    });
    return h("th", { scope: "col", class: cls, "aria-sort": on ? v.direction : null, id: `sort-${sortKey}` }, button);
  };
  // The same orders as a list, for narrow screens, where the columns' names aren't shown.
  const sorter = h(
    "select",
    { id: "make-room-sort" },
    Object.entries(SORTS).flatMap(([k, s]) =>
      !s.by
        ? [h("option", { value: `${k}:descending`, selected: v.sort === k }, s.name)]
        : ["descending", "ascending"].map((d) =>
            h("option", { value: `${k}:${d}`, selected: v.sort === k && v.direction === d }, `${s.name}, ${sortWords(k, d)}`),
          ),
    ),
  );
  sorter.addEventListener("change", () => {
    [v.sort, v.direction] = sorter.value.split(":");
    redraw(v, "make-room-sort");
  });
  return [
    h(
      "div",
      { class: "make-room-head" },
      h("p", {}, `${plural(items.length, "suggestion", "suggestions")}, freeing ${size(total)} in all.`),
      h("label", { class: "make-room-all", for: "make-room-all" }, everything, "Choose all"),
      h("div", { class: "make-room-sort" }, h("label", { for: "make-room-sort" }, "Sort by"), sorter),
    ),
    h(
      "div",
      { class: "table-wrap stacks" },
      h(
        "table",
        { class: "data make-room" },
        h("caption", { class: "visually-hidden" }, "Titles suggested for making room"),
        h(
          "thead",
          {},
          h(
            "tr",
            {},
            h("th", { scope: "col", class: "tick" }, h("span", { class: "visually-hidden" }, "Choose")),
            header("Title", "title"),
            header("Suggested", "action"),
            header("Size", "size", "r"),
            header("Frees", "saves", "r"),
            header("Why"),
            header("Actions"),
          ),
        ),
        h("tbody", {}, rows),
      ),
    ),
  ];
}

const sortWords = (k, direction) => {
  if (k === "title" || k === "action") return direction === "ascending" ? "A to Z" : "Z to A";
  return direction === "ascending" ? "smallest first" : "largest first";
};

// The list drawn again in its new order, with focus back on what changed it.
function redraw(v, focus) {
  v.list.replaceChildren(...view(v));
  const target = document.getElementById(focus);
  (target?.querySelector("button") ?? target)?.focus();
}

function row(v, s) {
  const label = labelOf(s);
  const pick = h("input", { type: "checkbox", "aria-label": `Choose ${label}`, checked: v.chosen.has(key(s)) });
  pick.addEventListener("change", () => {
    if (pick.checked) v.chosen.add(key(s));
    else v.chosen.delete(key(s));
    v.tickedAll?.();
    drawBar(v);
  });
  const actions = h("div", { class: "row-actions" });
  for (const [i, o] of s.options.entries()) {
    const text = i === 0 ? BUTTONS[o.action] : `${BUTTONS[o.action]} instead (frees ${size(o.saves)})`;
    const aria = `${o.action === "delete" ? `Move ${label} to the trash` : o.action === "swap" ? `Swap ${label} for a smaller release` : `Compress ${label}`}, freeing about ${size(o.saves)}`;
    actions.append(h("button", { type: "button", class: "quiet", "aria-label": aria, onclick: () => confirmOne(v, actions, s, o.action) }, text));
  }
  const cell = (name, content, cls) => h("td", { "data-label": name, class: cls }, content);
  return h(
    "tr",
    {},
    h("td", { class: "tick" }, pick),
    h("th", { scope: "row" }, h("a", { href: `#/admin/${v.ctx.service.key}/title/f${s.id}` }, label)),
    cell("Suggested", ACTIONS[s.action] ?? s.action),
    cell("Size", size(s.size), "r"),
    cell("Frees", size(s.saves), "r"),
    cell("Why", s.reasons.map((r) => REASONS[r] ?? r).join(", ") || "—"),
    cell("", actions),
  );
}

// What doing `action` to `titles` asks before it's done.
function question(action, titles) {
  const one = titles.length === 1 ? labelOf(titles[0]) : null;
  const what = one ?? plural(titles.length, "title", "titles");
  const series = titles.some((s) => s.kind === "series");
  if (action === "delete") {
    const bytes = titles.reduce((sum, s) => sum + s.size, 0);
    const whose = one ? `all of ${one}'s files` : `all the files of ${what}`;
    return `Move ${whose} (${size(bytes)}) to the trash? They're deleted in 30 days unless put back.`;
  }
  if (action === "swap") {
    return `Look for a smaller release of ${what}, at most 1080p, and replace ${one ? "its copy" : "each copy"} once it's in (the old one goes to the trash for 30 days)? ${one ? "It stops" : "They stop"} being upgraded.${series ? " Each episode is searched on its own, a few a minute." : ""}`;
  }
  return `Convert the H.264 files of ${what} to HEVC, one at a time while nothing else is converted, each copy replacing its file once it plays as long with the same tracks (the old file goes to the trash for 30 days)?`;
}

const CONFIRMS = { delete: "Move to trash", swap: "Swap", compress: "Compress" };

// Does `action` to each of `titles` in turn; what was done, and what wasn't and why, in words.
async function perform(action, titles) {
  const failures = [];
  let files = 0;
  for (const s of titles) {
    try {
      const done = await api(ROUTES[action], { method: "POST", body: { kind: s.kind, id: s.id } });
      files += done.files ?? 0;
    } catch (err) {
      if (err instanceof SignedOut) throw err;
      failures.push(`${labelOf(s)}: ${err.message}`);
    }
  }
  const done = titles.length - failures.length;
  const what = done === 1 && titles.length === 1 ? labelOf(titles[0]) : plural(done, "title", "titles");
  const verb = { delete: "moved to the trash", swap: "queued to swap for a smaller release", compress: "queued to compress" }[action];
  const head = done ? `${what}: ${plural(files, "file", "files")} ${verb}.` : "";
  return `${head}${failures.length ? `${head ? " " : ""}Not done: ${failures.join("; ")}` : ""}`;
}

function confirmOne(v, actions, s, action) {
  confirmInline(actions, {
    question: question(action, [s]),
    confirm: CONFIRMS[action],
    run: async () => {
      const words = await perform(action, [s]);
      v.chosen.delete(key(s));
      await acted(v, words);
    },
  });
}

// The bar for what's ticked: what doing what's suggested for each frees, and each action that
// some of them can take, for those that can. It stays at the foot of the screen.
function drawBar(v) {
  const picked = v.items.filter((s) => v.chosen.has(key(s)));
  v.bar.hidden = !picked.length && !v.said.textContent;
  if (v.bar.hidden) return;
  const actions = h("div", { class: "row-actions" });
  const ask = (label, action, titles) => {
    const button = h("button", { type: "button", class: "quiet" }, label);
    button.addEventListener("click", () =>
      confirmInline(actions, {
        question: action ? question(action, titles) : suggestedQuestion(titles),
        confirm: label,
        run: async () => {
          const words = action ? await perform(action, titles) : await performSuggested(titles);
          for (const s of titles) v.chosen.delete(key(s));
          await acted(v, words);
        },
      }),
    );
    return button;
  };
  if (picked.length) {
    const frees = picked.reduce((sum, s) => sum + s.saves, 0);
    if (new Set(picked.map((s) => s.action)).size > 1) actions.append(ask(`Do what's suggested for ${plural(picked.length, "title", "titles")} (${size(frees)})`, null, picked));
    for (const action of ["delete", "swap", "compress"]) {
      const can = picked.filter((s) => optionOf(s, action));
      if (!can.length) continue;
      const bytes = can.reduce((sum, s) => sum + optionOf(s, action).saves, 0);
      const count = can.length === picked.length ? plural(can.length, "title", "titles") : `${can.length} of ${picked.length}`;
      const name = { delete: `Move ${count} to the trash`, swap: `Swap ${count}`, compress: `Compress ${count}` }[action];
      actions.append(ask(`${name} (${size(bytes)})`, action, can));
    }
    const clear = h("button", { type: "button", class: "quiet" }, "Clear selection");
    clear.addEventListener("click", () => {
      v.chosen.clear();
      for (const box of v.list.querySelectorAll("tbody .tick input")) box.checked = false;
      v.tickedAll?.();
      v.said.textContent = "";
      drawBar(v);
      everythingBox(v)?.focus();
    });
    actions.append(clear);
  }
  const count = picked.length ? h("strong", {}, `${plural(picked.length, "title", "titles")} chosen, freeing ${size(picked.reduce((sum, s) => sum + s.saves, 0))} as suggested`) : "";
  v.said.tabIndex = -1;
  v.bar.replaceChildren(count, actions, v.said);
}

const everythingBox = (v) => v.list.querySelector("#make-room-all");

function suggestedQuestion(titles) {
  const by = (action) => titles.filter((s) => s.action === action).length;
  const parts = [
    by("delete") && `move ${plural(by("delete"), "title", "titles")} to the trash`,
    by("swap") && `swap ${plural(by("swap"), "title", "titles")} for smaller releases`,
    by("compress") && `compress ${plural(by("compress"), "title", "titles")}`,
  ].filter(Boolean);
  return `Do what's suggested for each: ${parts.join(", ")}? Whatever is moved or replaced goes to the trash for 30 days.`;
}

async function performSuggested(titles) {
  const words = [];
  for (const action of ["delete", "swap", "compress"]) {
    const these = titles.filter((s) => s.action === action);
    if (these.length) words.push(await perform(action, these));
  }
  return words.filter(Boolean).join(" ");
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

