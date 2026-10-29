// The rules every page follows, as pieces to build with (styles in shell.css):
//
// - placeholders(what, { kind, count }): while a list loads, grey shapes the size of what's
//   coming ("rows", "cards" or "lines"), so nothing jumps when it arrives. Screen readers hear
//   "Loading {what}…" instead. Swap it for the content in one replaceChildren.
// - unlessEmpty(items, build): a row with nothing in it is left out: null for an empty list,
//   else build(items).
// - emptyState(sentence, way): a page with nothing in it says so in one plain sentence, with
//   one way forward: { href, label } for a link, or { label, onclick } for a button.
// - failed(what, err, retry): what failed and why, with Try again. Announced as an alert.
// - clip(text, tag): a long name on one line, cut with "…", shown in full on hover.
// - hold(button) / release(button): a button busy with what it started is marked
//   aria-disabled and further presses are ignored (hold answers false), but it isn't disabled,
//   since a disabled button loses the keyboard's focus; release() frees it.
// - stateLabel(state, words): the managing side's states, each in words with a mark of its own
//   shape and colour (have, coming, waiting, missing, failed, off, notyet); stateKey(states)
//   lists them as a key under a list that uses them.

import { h } from "/web/lib.js";

const KINDS = { rows: 6, cards: 12, lines: 3 };

export function placeholders(what, { kind = "rows", count = KINDS[kind] } = {}) {
  return h(
    "div",
    { class: `placeholders ${kind}`, role: "status", "aria-busy": "true" },
    h("span", { class: "visually-hidden" }, `Loading ${what}…`),
    Array.from({ length: count }, () => h("div", { class: "placeholder", "aria-hidden": "true" })),
  );
}

export function unlessEmpty(items, build) {
  return items?.length ? build(items) : null;
}

export function emptyState(sentence, way) {
  let forward = null;
  if (way?.href) forward = h("a", { class: "button quiet", href: way.href }, way.label);
  else if (way?.onclick) forward = h("button", { type: "button", class: "quiet", onclick: way.onclick }, way.label);
  return h("div", { class: "empty-state" }, h("p", {}, sentence), forward);
}

export function failed(what, err, retry) {
  return h(
    "div",
    { class: "failed", role: "alert" },
    h("p", { class: "error" }, `Couldn't load ${what}. ${err.message}`),
    retry ? h("button", { type: "button", class: "quiet", onclick: retry }, "Try again") : null,
  );
}

export function hold(button) {
  if (button.getAttribute("aria-disabled") === "true") return false;
  button.setAttribute("aria-disabled", "true");
  return true;
}

export function release(button) {
  button.removeAttribute("aria-disabled");
}

export function clip(text, tag = "span") {
  return h(tag, { class: "clip", title: text }, text);
}

// The words each state goes by when a page doesn't say more.
export const STATES = {
  have: "In the library",
  coming: "On its way",
  waiting: "Waiting for a decision",
  missing: "Missing",
  failed: "Failed",
  off: "Not monitored",
  notyet: "Not out yet",
};

export function stateLabel(state, words = STATES[state]) {
  return h("span", { class: `state state-${state}` }, h("span", { class: "mark", "aria-hidden": "true" }), words);
}

export function stateKey(states) {
  return h("ul", { class: "state-key", "aria-label": "Key" }, states.map((s) => h("li", {}, stateLabel(s))));
}
