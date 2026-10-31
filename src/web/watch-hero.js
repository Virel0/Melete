// The hero at the top of Watch's pages: one title large (its textless backdrop cropped around
// its subject, logo or title, facts, logline, and Play saying what it will do, More info and
// My List), stepping between picks every 16 seconds or by its buttons. It holds still while
// pointed at or focused, or while a pick's trailer plays (watch-hero-trailer.js), can be paused,
// and doesn't move by itself for those who ask for less motion.
//
// hero(picks, { watching }) → { el, spotlight, stop }:
// - `watching` maps a series id to the row entry that names its episode (partway through, or
//   next), so Play on that series goes there.
// - `spotlight.add(items)` says which titles the tiles are; `spotlight.watch(list)` lets a
//   row's tiles show their title here after a moment's hover or focus, going back to the pick
//   a moment after they're left.
// - `stop()` ends the stepping and any trailer, for leaving the page.

import { SignedOut, announce, h, runtime } from "/web/lib.js";
import { signIn } from "/web/page-state.js";
import { myListToggle } from "/web/favourites.js";
import { icon } from "/web/music-controls.js";
import { heroTrailer } from "/web/watch-hero-trailer.js";
import { partway, playTarget, titleHref } from "/web/watch-play.js";

const STEP_MS = 16_000;
// Mneme's timings: follow a tile after 160 ms, so sweeping along a row doesn't change the hero
// six times; go back 300 ms after it's left, so moving between tiles doesn't flash the pick.
const FOLLOW_MS = 160;
const RETURN_MS = 300;

const stillness = matchMedia("(prefers-reduced-motion: reduce)");

// What the hero says about a title under its name: kind, year, two genres, a movie's length.
function facts(item) {
  return [
    item.kind === "movie" ? "Movie" : "Series",
    item.year ? String(item.year) : null,
    item.genres?.slice(0, 2).join(", ") || null,
    item.kind === "movie" ? runtime(item.runtime_minutes) : null,
  ].filter(Boolean);
}

export function hero(picks, { watching = new Map() } = {}) {
  const enrich = (item) => {
    const w = item.kind === "series" && !item.episode ? watching.get(item.id) : null;
    return w ? { ...item, episode: w.episode, progress: w.progress ?? item.progress } : item;
  };
  const p = {
    picks: picks.map(enrich),
    at: 0,
    shown: null,
    art: h("img", { alt: "", class: "hero-picture" }),
    kicker: h("p", { class: "hero-kicker" }),
    title: h("h2", { class: "hero-title" }),
    facts: h("ul", { class: "meta-row hero-facts" }),
    logline: h("p", { class: "hero-logline" }),
    play: h("a", { class: "button hero-play", "aria-disabled": "true" }, icon("play"), "Play"),
    info: h("a", { class: "button quiet" }, "More info"),
    list: myListToggle(),
    held: false,
    paused: stillness.matches,
  };
  const art = h("div", { class: "hero-art" }, p.art);
  p.trailer = heroTrailer(art);
  const steps = stepControls(p);
  p.el = h(
    "section",
    { class: "hero", "aria-roledescription": "carousel", "aria-label": "Picks" },
    art,
    h(
      "div",
      { class: "hero-body" },
      p.kicker,
      p.title,
      p.facts,
      p.logline,
      h("div", { class: "actions" }, p.play, p.info, p.list.el),
      p.trailer.el,
      steps.el,
    ),
  );
  p.el.addEventListener("pointerenter", () => (p.held = true));
  p.el.addEventListener("pointerleave", () => (p.held = p.el.contains(document.activeElement)));
  p.el.addEventListener("focusin", () => (p.held = true));
  p.el.addEventListener("focusout", (event) => (p.held = p.el.contains(event.relatedTarget)));
  if (p.picks.length) draw(p, p.picks[0]);
  const timer = setInterval(() => {
    if (p.paused || p.held || p.trailer.playing() || document.hidden || p.picks.length < 2 || p.shown !== p.picks[p.at]) return;
    p.at = (p.at + 1) % p.picks.length;
    draw(p, p.picks[p.at]);
    steps.mark();
  }, STEP_MS);
  const stop = () => {
    clearInterval(timer);
    p.trailer.stop();
  };
  return { el: p.el, spotlight: spotlight(p, enrich), stop };
}

// Previous, a dot per pick, next, and Pause (left out when motion is turned down, since the
// picks don't move by themselves then); nothing for a single pick.
function stepControls(p) {
  const count = p.picks.length;
  const dots = p.picks.map((item, i) =>
    // Out of the Tab order: Previous and Next step through the same picks with fewer stops.
    h("button", { type: "button", class: "hero-dot", tabindex: "-1", "aria-label": `Pick ${i + 1} of ${count}: ${item.title}` }, h("span", {})),
  );
  const mark = () => dots.forEach((d, i) => d.setAttribute("aria-current", String(i === p.at)));
  const go = (i) => {
    p.at = (i + count) % count;
    draw(p, p.picks[p.at]);
    mark();
    announce(`Pick ${p.at + 1} of ${count}: ${p.picks[p.at].title}`);
  };
  dots.forEach((d, i) => d.addEventListener("click", () => go(i)));
  const pause = stillness.matches ? null : h("button", { type: "button", class: "quiet hero-pause", "aria-pressed": "false" }, "Pause");
  pause?.addEventListener("click", () => {
    p.paused = !p.paused;
    pause.setAttribute("aria-pressed", String(p.paused));
  });
  mark();
  if (count < 2) return { el: null, mark };
  const el = h(
    "div",
    { class: "hero-steps" },
    h("button", { type: "button", class: "icon hero-step", "aria-label": "Previous pick", onclick: () => go(p.at - 1) }, icon("left")),
    h("div", { class: "hero-dots" }, dots),
    h("button", { type: "button", class: "icon hero-step", "aria-label": "Next pick", onclick: () => go(p.at + 1) }, icon("right")),
    pause,
  );
  return { el, mark };
}

// Puts `item` in the hero: a pick, or a tile's title.
function draw(p, item) {
  if (!item || p.shown === item) return;
  p.shown = item;
  const back = item.textless_backdrop ?? item.backdrop;
  const focus = item.textless_backdrop ? item.textless_backdrop_focus : item.backdrop_focus;
  p.el?.classList.toggle("no-art", !back);
  p.art.classList.remove("shown");
  if (back) {
    p.art.style.setProperty("object-position", focus ? `${focus.x * 100}% ${focus.y * 100}%` : "50% 30%");
    p.art.addEventListener("load", () => p.art.classList.add("shown"), { once: true });
    p.art.src = back;
  } else {
    p.art.removeAttribute("src");
  }
  p.kicker.textContent = partway(item.progress) ? "Continue watching" : "";
  p.title.replaceChildren(item.logo ? h("img", { src: item.logo, alt: item.title, class: "hero-logo" }) : item.title);
  p.facts.replaceChildren(...facts(item).map((f) => h("li", {}, f)));
  p.logline.textContent = item.overview ?? "";
  p.info.href = titleHref(item);
  p.info.setAttribute("aria-label", `More info: ${item.title}`);
  p.list.show(item);
  aimPlay(p, item);
  // A pick's trailer, after a few seconds on it; a tile's title shown here plays none.
  if (p.picks.includes(item)) p.trailer.show(item);
  else p.trailer.stop();
}

// Play, once what it does is known; gone when there's nothing to play.
function aimPlay(p, item) {
  const set = (target) => {
    if (p.shown !== item) return;
    p.play.hidden = !target;
    if (!target) return;
    p.play.href = target.href;
    p.play.removeAttribute("aria-disabled");
    p.play.replaceChildren(icon("play"), target.label);
    p.play.setAttribute("aria-label", `${target.label}: ${item.title}`);
  };
  p.play.hidden = false;
  p.play.removeAttribute("href");
  p.play.removeAttribute("aria-label");
  p.play.setAttribute("aria-disabled", "true");
  p.play.replaceChildren(icon("play"), "Play");
  const target = playTarget(item);
  if (target instanceof Promise) {
    target.then(set).catch((err) => {
      if (err instanceof SignedOut) signIn("Your session ended. Sign in again.");
    });
  } else set(target);
}

// Tiles that show their title in the hero after a moment's hover or focus.
function spotlight(p, enrich) {
  const items = new Map();
  let follow;
  let back;
  const hold = (tile) => {
    clearTimeout(back);
    clearTimeout(follow);
    const item = items.get(tile.dataset.title);
    if (item) follow = setTimeout(() => draw(p, item), FOLLOW_MS);
  };
  const leave = (next) => {
    clearTimeout(follow);
    // Into the hero, or onto another tile: keep what's shown.
    if (next && (p.el.contains(next) || next.closest?.("a.tile"))) return;
    back = setTimeout(() => draw(p, p.picks[p.at]), RETURN_MS);
  };
  const tileOf = (event) => event.target.closest?.("a.tile");
  return {
    add(list) {
      for (const item of list) {
        const key = `${item.kind}:${item.id}`;
        if (!items.has(key)) items.set(key, enrich(item));
      }
    },
    watch(row) {
      row.addEventListener("pointerover", (event) => tileOf(event) && hold(tileOf(event)));
      row.addEventListener("pointerleave", () => leave(document.activeElement?.closest?.("a.tile") ? document.activeElement : null));
      row.addEventListener("focusin", (event) => tileOf(event) && hold(tileOf(event)));
      row.addEventListener("focusout", (event) => leave(event.relatedTarget));
    },
  };
}
