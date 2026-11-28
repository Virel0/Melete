// What needs attention in a service, from the admin health (health.js), as cards at the top of
// its first page: each finding's first sentence in bold, the rest of it softer, and one button
// to where it's dealt with (left out on that page itself). A problem sits on the danger tint with
// the danger fill on its button; anything else on the surface with a secondary button.
//
// A finding the server makes once per title (a movie at a made-up frame rate, an anime series
// without styled subtitles) is one card for all of them, saying how many, with the titles in a
// fold under it: six at most, by name, each a link, and "and N more". Their names are asked for
// when the fold is opened.
//
// attentionStrip(findings, here) → the section, or null when there's nothing to say.

import { SignedOut, api, h, plural } from "/web/lib.js";
import { signIn } from "/web/page-state.js";

const ORDER = { problem: 0, waiting: 1, note: 2 };
const SHOWN = 6;

// The findings made once per title, said once for many: the words, and where a title's name is
// read from.
const PER_TITLE = {
  movies_high_frame_rate: {
    said: (n) => [
      `${plural(n, "movie plays", "movies play")} at a frame rate likely made up from the film's own.`,
      `${n === 1 ? "It" : "They"} may stutter on some players. Search for another release.`,
    ],
    name: (id) => api(`/movies/${id}`).then((m) => m.title),
  },
  anime_unstyled_subtitles: {
    said: (n) => [
      `${plural(n, "anime series has", "anime series have")} episodes without styled subtitles.`,
      "Their subtitles show as plain words or not at all. Search for a release that has them.",
    ],
    name: (id) => api(`/series/${id}`).then((s) => s.title),
  },
};

// "Sixty plays at 60 fps. Search for another release." → ["Sixty plays at 60 fps.", "Search …"]
function sentences(text) {
  const at = text.search(/[.:;] /);
  return at < 0 ? [text, ""] : [text.slice(0, at + 1), text.slice(at + 2)];
}

// The titles behind one card, in a fold that reads their names when it's first opened.
function titlesFold(code, items) {
  const list = h("ul", { class: "attention-titles" });
  const fold = h("details", { class: "attention-fold" }, h("summary", {}, `Which ${items.length === 1 ? "one" : "ones"}`), list);
  fold.addEventListener("toggle", async () => {
    if (!fold.open || list.childElementCount) return;
    const shown = items.slice(0, SHOWN);
    const names = await Promise.all(
      shown.map((f) =>
        PER_TITLE[code].name(f.id).catch((err) => {
          if (err instanceof SignedOut) signIn("Your session ended. Sign in again.");
          return `Title ${f.id}`;
        }),
      ),
    );
    list.replaceChildren(...shown.map((f, i) => h("li", {}, h("a", { href: f.href }, names[i]))));
    if (items.length > SHOWN) list.append(h("li", { class: "attention-more" }, `and ${items.length - SHOWN} more`));
  });
  return fold;
}

// One card: bold words, softer ones, a fold of titles if any, and one button.
function card(level, [bold, soft], href, here, fold = null) {
  const go = href && href !== here ? h("a", { class: `button ${level === "problem" ? "danger" : "quiet"}`, href }, level === "note" ? "Open" : "Deal with it") : null;
  return h(
    "li",
    { class: `attention-card level-${level}` },
    h("div", { class: "attention-words" }, h("strong", {}, bold), soft ? h("span", { class: "attention-why" }, soft) : null, fold),
    go,
  );
}

export function attentionStrip(findings, here) {
  if (!findings?.length) return null;
  const sorted = [...findings].sort((a, b) => ORDER[a.level] - ORDER[b.level]);
  const cards = [];
  const grouped = new Map();
  for (const f of sorted) {
    if (PER_TITLE[f.code] && f.id !== null && f.id !== undefined) {
      if (!grouped.has(f.code)) {
        grouped.set(f.code, []);
        cards.push({ code: f.code, level: f.level });
      }
      grouped.get(f.code).push(f);
    } else {
      cards.push({ finding: f });
    }
  }
  return h(
    "section",
    { class: "attention", "aria-labelledby": "attention-title" },
    h("h2", { id: "attention-title" }, "Needs attention"),
    h(
      "ul",
      {},
      cards.map((c) => {
        if (c.finding) return card(c.finding.level, sentences(c.finding.text), c.finding.href, here);
        const items = grouped.get(c.code);
        if (items.length === 1) return card(c.level, sentences(items[0].text), items[0].href, here);
        return card(c.level, PER_TITLE[c.code].said(items.length), null, here, titlesFold(c.code, items));
      }),
    ),
  );
}
