// The smart playlist rule editor: how the rules combine (all or any), each rule's field, how it
// compares and with what, the order and how many songs at most, in the Aoide apps' rule format.
// What the rules give is shown as they're changed, worked out by the server for the listener
// (`POST /smart-playlists/songs`). Used for a new smart playlist on the playlists page and for
// Edit rules on one's own smart playlist.

import { SignedOut, api, h, plural } from "/web/lib.js";
import { playlistTrouble, playlistsChanged } from "/web/music-playlist-pick.js";
import { hold, release } from "/web/page-parts.js";

// Each field, its name in words and its kind, in the apps' order.
const FIELDS = [
  ["title", "Title", "text"],
  ["artist", "Artist", "text"],
  ["album", "Album", "text"],
  ["album_artist", "Album artist", "text"],
  ["genre", "Genre", "text"],
  ["year", "Year", "number"],
  ["duration_ms", "Length (seconds)", "number"],
  ["play_count", "Plays", "number"],
  ["skip_count", "Skips", "number"],
  ["last_played", "Last played", "date"],
  ["first_played", "First played", "date"],
  ["liked", "Liked", "flag"],
];
const KIND = Object.fromEntries(FIELDS.map(([f, , k]) => [f, k]));
// What each kind compares with, in words.
const OPERATORS = {
  text: [["is", "is"], ["isNot", "is not"], ["contains", "contains"], ["doesNotContain", "doesn't contain"], ["beginsWith", "begins with"]],
  number: [["is", "is"], ["isNot", "is not"], ["greaterThan", "is more than"], ["lessThan", "is less than"]],
  date: [["inTheLast", "in the last"], ["notInTheLast", "not in the last"], ["before", "more than … ago"], ["after", "less than … ago"]],
  flag: [["is", "is"], ["isNot", "is not"]],
};
const UNITS = [["d", "days"], ["w", "weeks"], ["m", "months"], ["y", "years"], ["h", "hours"]];
// Operators written as symbols are read by name, as the server reads them.
const SYMBOLS = { "=": "is", "==": "is", "!=": "isNot", "<>": "isNot", ">": "greaterThan", "<": "lessThan" };

let made = 0;
// How long typing settles before the preview is asked again.
const SETTLE_MS = 400;

// One rule's row: field, operator, value and Remove. `changed` is called on every edit.
function ruleRow(rule, changed, remove) {
  const id = `rule-${++made}`;
  const field = h("select", { id: `${id}-field`, "aria-label": "Field" }, FIELDS.map(([f, label]) => h("option", { value: f, selected: f === rule.field }, label)));
  const op = h("select", { id: `${id}-op`, "aria-label": "How it compares" });
  const valueBox = h("span", { class: "rule-value" });
  let read = () => null;
  const drawValue = (value) => {
    const kind = KIND[field.value];
    if (kind === "flag") {
      const box = h("select", { "aria-label": "Value" }, h("option", { value: "true", selected: value !== false }, "yes"), h("option", { value: "false", selected: value === false }, "no"));
      box.addEventListener("change", changed);
      valueBox.replaceChildren(box);
      read = () => box.value === "true";
    } else if (kind === "date") {
      const [, amount = "30", unit = "d"] = /^-?(\d+)([hdwmy])$/i.exec(String(value ?? "")) ?? [];
      const n = h("input", { type: "number", min: "0", value: amount, class: "year", "aria-label": "How many" });
      const u = h("select", { "aria-label": "Of what" }, UNITS.map(([key, label]) => h("option", { value: key, selected: key === unit.toLowerCase() }, label)));
      n.addEventListener("input", changed);
      u.addEventListener("change", changed);
      valueBox.replaceChildren(n, u);
      read = () => `-${Math.max(0, Math.trunc(Number(n.value) || 0))}${u.value}`;
    } else {
      const seconds = field.value === "duration_ms";
      const shown = kind === "number" && value != null ? (seconds ? Math.round(value / 1000) : value) : (value ?? "");
      const input = h("input", { type: kind === "number" ? "number" : "text", value: shown, "aria-label": "Value", class: kind === "number" ? "year" : null });
      input.addEventListener("input", changed);
      valueBox.replaceChildren(input);
      read = () => (kind === "number" ? Math.trunc(Number(input.value) || 0) * (seconds ? 1000 : 1) : input.value);
    }
  };
  const drawOps = (wanted) => {
    const ops = OPERATORS[KIND[field.value]];
    const pick = ops.some(([o]) => o === wanted) ? wanted : ops[0][0];
    op.replaceChildren(...ops.map(([o, label]) => h("option", { value: o, selected: o === pick }, label)));
  };
  drawOps(SYMBOLS[rule.op] ?? rule.op);
  drawValue(rule.value);
  field.addEventListener("change", () => {
    drawOps(op.value);
    drawValue(null);
    changed();
  });
  op.addEventListener("change", changed);
  const row = h(
    "li",
    { class: "rule-row" },
    field,
    op,
    valueBox,
    h("button", { type: "button", class: "quiet small", onclick: () => remove(row) }, "Remove"),
  );
  row.read = () => ({ field: field.value, op: op.value, value: read() });
  return row;
}

// The editor. `rules` to start from (none for a new playlist); `save(rules, name)` keeps them and
// returns what to say; `cancel` closes it. A new playlist also asks for a name.
export function smartEditor({ rules = null, named = false, save, cancel }) {
  const id = `smart-${++made}`;
  const start = rules ?? { match: "all", rules: [{ field: "liked", op: "is", value: true }] };
  const name = named ? h("input", { id: `${id}-name`, maxlength: "200", autocomplete: "off", required: true }) : null;
  const match = h("select", { id: `${id}-match` }, h("option", { value: "all", selected: start.match !== "any" }, "every rule"), h("option", { value: "any", selected: start.match === "any" }, "any rule"));
  const list = h("ul", { class: "rule-list", "aria-label": "Rules" });
  const sortable = FIELDS.filter(([f]) => f !== "genre");
  const sortField = h("select", { id: `${id}-sort` }, h("option", { value: "" }, "Title (A to Z)"), sortable.map(([f, label]) => h("option", { value: f, selected: start.sort?.field === f }, label)));
  const sortDir = h("select", { "aria-label": "Which way" }, h("option", { value: "asc", selected: start.sort?.dir !== "desc" }, "smallest or A first"), h("option", { value: "desc", selected: start.sort?.dir === "desc" }, "largest or Z first"));
  const limit = h("input", { id: `${id}-limit`, type: "number", min: "1", class: "year", value: start.limit ?? "" });
  const preview = h("div", { class: "smart-preview", "aria-live": "polite" });
  const said = h("p", { class: "note", role: "status" });
  const keep = h("button", { type: "submit" }, named ? "Make smart playlist" : "Save rules");

  const current = () => {
    const out = { match: match.value, rules: [...list.children].map((row) => row.read()) };
    if (sortField.value) out.sort = { field: sortField.value, dir: sortDir.value };
    const most = Math.trunc(Number(limit.value));
    if (most > 0) out.limit = most;
    return out;
  };
  let timer = null;
  let asking = null;
  const ask = () => {
    clearTimeout(timer);
    timer = setTimeout(async () => {
      asking?.abort();
      const mine = (asking = new AbortController());
      try {
        const found = await api("/smart-playlists/songs", { method: "POST", body: { rules: current(), offset_minutes: -new Date().getTimezoneOffset() }, signal: mine.signal });
        if (mine !== asking) return;
        // The first few songs, and how many more.
        const first = found.tracks.slice(0, 10).map((t) => h("li", {}, h("strong", {}, t.title), ` · ${t.artist ?? t.album_artist}`));
        preview.replaceChildren(
          h(
            "div",
            {},
            h("p", {}, found.count ? `${plural(found.count, "song fits", "songs fit")} now.` : "No song fits now."),
            first.length ? h("ol", { class: "plain smart-preview-list" }, first) : null,
            found.count > 10 ? h("p", { class: "note" }, `and ${plural(found.count - 10, "more", "more")}.`) : null,
          ),
        );
      } catch (err) {
        if (err.name === "AbortError") return;
        if (err instanceof SignedOut) throw err;
        preview.replaceChildren(h("p", { class: "error" }, err.message));
      }
    }, SETTLE_MS);
  };
  const removeRow = (row) => {
    const next = row.nextElementSibling ?? row.previousElementSibling;
    row.remove();
    (next?.querySelector("select") ?? add).focus();
    ask();
  };
  const addRow = (rule) => list.append(ruleRow(rule, ask, removeRow));
  for (const rule of start.rules) addRow(rule);
  const add = h("button", { type: "button", class: "quiet small" }, "Add rule");
  add.addEventListener("click", () => {
    addRow({ field: "title", op: "contains", value: "" });
    list.lastElementChild.querySelector("select").focus();
    ask();
  });
  for (const el of [match, sortField, sortDir]) el.addEventListener("change", ask);
  limit.addEventListener("input", ask);

  const form = h(
    "form",
    { class: "smart-editor", novalidate: true },
    named ? h("div", { class: "field" }, h("label", { for: `${id}-name` }, "Name"), name) : null,
    h("div", { class: "field" }, h("label", { for: `${id}-match` }, "Songs that meet"), match),
    list,
    h("div", { class: "form-actions" }, add),
    h(
      "div",
      { class: "smart-order" },
      h("div", { class: "field" }, h("label", { for: `${id}-sort` }, "In order of"), sortField, sortDir),
      h("div", { class: "field" }, h("label", { for: `${id}-limit` }, "At most (songs)"), limit),
    ),
    h("h3", {}, "What it gives you"),
    preview,
    h("div", { class: "form-actions" }, keep, h("button", { type: "button", class: "quiet", onclick: cancel }, "Cancel")),
    said,
  );
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    if (named && !name.value.trim()) {
      said.textContent = "Give it a name.";
      name.focus();
      return;
    }
    if (!hold(keep)) return;
    try {
      said.textContent = (await save(current(), name?.value.trim())) ?? "";
      playlistsChanged();
    } catch (err) {
      if (err instanceof SignedOut) throw err;
      said.textContent = `Couldn't save it: ${playlistTrouble(err)}`;
    } finally {
      release(keep);
    }
  });
  ask();
  return form;
}
