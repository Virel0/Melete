// Art drawn for a title or an album that has none, instead of a blank grey square: a dark
// colour taken from its name, with its initials in a lighter colour of the same hue at the
// bottom left, and its full name under them when `named`. The same name always gets the same
// colour, so a title is known by it from page to page. Screen readers skip it: the name is
// beside it.
//
// drawnArt(name, { named }) → the element; watch.css draws it.

import { h } from "/web/lib.js";

// A hue from the name's letters, 0–359.
export function hueOf(name) {
  let n = 0;
  for (const ch of name) n = (n * 31 + ch.codePointAt(0)) % 360;
  return n;
}

// "The Long Field" → "LF", "Brightwater" → "B": the first letters of its first two words.
export function initialsOf(name) {
  const words = name.match(/[\p{L}\p{N}][\p{L}\p{N}'’]*/gu) ?? [];
  const kept = words.length > 2 && /^(the|a|an)$/i.test(words[0]) ? words.slice(1) : words;
  return kept
    .slice(0, 2)
    .map((w) => w[0].toUpperCase())
    .join("");
}

export function drawnArt(name, { named = false } = {}) {
  const el = h(
    "span",
    { class: "drawn-art", "aria-hidden": "true" },
    h("span", { class: "initials" }, initialsOf(name) || "?"),
    named ? h("span", { class: "drawn-name" }, name) : null,
  );
  el.style.setProperty("--art-hue", String(hueOf(name)));
  return el;
}
