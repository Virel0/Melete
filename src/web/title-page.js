// A movie's or a series' own page: a header on the stage (its textless backdrop, logo or title,
// facts, overview and genres, then Play saying what it will do, Mark watched and My List), the
// page fading down from the title's strongest colour, which Play wears too. A series then has
// its seasons and episodes (title-episodes.js); extras, cast and the collection have their own
// addresses, and About folds the file details away (title-more.js).
//
// tint(src, known) stays for the album pages, which tint flat with a cover's colour.

import { SignedOut, api, h, plural, runtime, show } from "/web/lib.js";
import { generation, leaving } from "/web/page-state.js";
import { playHref } from "/web/cards.js";
import { myListToggle } from "/web/favourites.js";
import { icon } from "/web/music-controls.js";
import { placeholders } from "/web/page-parts.js";
import { buttonColour, css, strongest, tintPage } from "/web/title-colour.js";
import { episodesPicker, markFiles, markSeries, seasonOrder } from "/web/title-episodes.js";
import { aboutSection, castPage, extrasPage, moreLikeThis, moreLinks } from "/web/title-more.js";
import { trailerButton } from "/web/title-trailer.js";
import { partway, playTarget, timeLeft } from "/web/watch-play.js";

// Tints the page flat with an image's colour (an album's cover), darkened first so text keeps
// its contrast over it. An album brings its cover's colour, worked out on the server.
export function tint(src, known) {
  const set = (avg) => {
    const scale = 120 / Math.max(...avg, 1);
    const [r, g, b] = avg.map((v) => Math.round(v * scale));
    const colour = `rgb(${r} ${g} ${b})`; // check-ui: ignore computed from the poster
    document.documentElement.style.setProperty("--tint", colour);
  };
  leaving.push(() => document.documentElement.style.removeProperty("--tint"));
  const hex = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(known || "");
  if (hex) return set(hex.slice(1).map((x) => parseInt(x, 16)));
  if (!src) return;
  const img = new Image();
  img.addEventListener("load", () => {
    const canvas = document.createElement("canvas");
    canvas.width = canvas.height = 16;
    const g = canvas.getContext("2d", { willReadFrequently: true });
    g.drawImage(img, 0, 0, 16, 16);
    const data = g.getImageData(0, 0, 16, 16).data;
    const sum = [0, 0, 0];
    for (let i = 0; i < data.length; i += 4) for (let c = 0; c < 3; c++) sum[c] += data[i + c];
    set(sum.map((v) => v / (data.length / 4)));
  });
  img.src = src;
}

// `#/movies/5`, `#/series/3?season=2`, and their `/extras` and `/cast`.
export function titlePage(kind, id, under, params, gen, admin) {
  const which = kind === "movies" ? "movie" : "series";
  if (under === "extras") return extrasPage(which, id, gen);
  if (under === "cast") return castPage(which, id, gen);
  return which === "movie" ? movie(id, gen, admin) : series(id, params, gen, admin);
}

// The stage's ink, which Play's words are in, as [r, g, b].
function stageInk() {
  const hex = getComputedStyle(document.documentElement).getPropertyValue("--stage-accent-ink").trim();
  const m = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(hex);
  return m ? m.slice(1).map((x) => parseInt(x, 16)) : [0, 0, 0];
}

const MATCH_NOTES = {
  ambiguous: "TMDB has more than one title this could be, so the name comes from the folder. An admin can pick the right one.",
  unmatched: "Not found on TMDB, so the name comes from the folder.",
};

// The header: art, words, and the actions given. Returns { el, play } so Play can be aimed
// once what it does is known.
function header(t, facts, genreHref, actions) {
  const art = t.textless_backdrop ?? t.backdrop;
  const focus = t.textless_backdrop ? t.textless_backdrop_focus : t.backdrop_focus;
  const picture = art ? h("img", { class: "hero-picture shown", src: art, alt: "" }) : null;
  picture?.style.setProperty("object-position", focus ? `${focus.x * 100}% ${focus.y * 100}%` : "50% 25%");
  const play = h("a", { class: "button hero-play", "aria-disabled": "true" }, icon("play"), "Play");
  const el = h(
    "section",
    { class: `hero title-hero${art ? "" : " no-art"}`, "aria-labelledby": "title-name" },
    h("div", { class: "hero-art" }, picture),
    h(
      "div",
      { class: "hero-body" },
      h("h1", { class: "hero-title", id: "title-name" }, t.logo ? h("img", { src: t.logo, alt: t.title, class: "hero-logo" }) : t.title),
      h("ul", { class: "meta-row hero-facts" }, facts.filter(Boolean).map((f) => h("li", {}, f))),
      t.overview ? h("p", { class: "title-overview" }, t.overview) : null,
      t.genres.length ? h("ul", { class: "pills", "aria-label": "Genres" }, t.genres.map((g) => h("li", {}, h("a", { href: genreHref(g) }, g)))) : null,
      h("div", { class: "actions" }, play, actions),
      MATCH_NOTES[t.match_status] ? h("p", { class: "note" }, MATCH_NOTES[t.match_status]) : null,
    ),
  );
  return { el, play };
}

// Points Play at `target` ({ href, label }), or hides it when there's nothing to play.
function aim(play, target, title) {
  play.hidden = !target;
  if (!target) return;
  play.href = target.href;
  play.removeAttribute("aria-disabled");
  play.replaceChildren(icon("play"), target.label);
  play.setAttribute("aria-label", `${target.label}: ${title}`);
}

// The page fades down from the strongest colour of its art, and Play wears it.
function colour(t, play, gen) {
  strongest(t.textless_backdrop ?? t.backdrop ?? t.poster).then((rgb) => {
    if (!rgb || gen !== generation) return;
    tintPage(rgb);
    play.classList.add("tinted");
    play.style.setProperty("--play-colour", css(buttonColour(rgb, stageInk())));
  });
}

// Progress entries by file id, for a movie or a series.
async function progressFor(kind, id) {
  const entries = await api(`/me/progress?${kind}_id=${id}`);
  return new Map(entries.map((e) => [e.file_id, e]));
}

const quiet = (promise, fallback) => promise.catch((err) => (err instanceof SignedOut ? Promise.reject(err) : fallback));

function loadingPage(what) {
  show(what, h("div", { class: "hero-placeholder placeholder", "aria-hidden": "true" }), placeholders(what.toLowerCase(), { kind: "lines" }));
}

export async function movie(id, gen, admin = false) {
  loadingPage("Movie");
  const [m, progress, extras, credits] = await Promise.all([
    api(`/movies/${id}`),
    progressFor("movie", id),
    quiet(api(`/movies/${id}/extras`), []),
    quiet(api(`/movies/${id}/credits`), { cast: [], crew: [], collection: null }),
  ]);
  if (gen !== generation) return;
  const list = myListToggle();
  list.show({ kind: "movie", id: m.id, title: m.title });
  const mark = h("button", { type: "button", class: "quiet" });
  const head = header(m, ["Movie", m.year && String(m.year), runtime(m.runtime_minutes)], (g) => `#/movies?genre=${encodeURIComponent(g)}`, [
    m.files.length ? mark : null,
    list.el,
    trailerButton("movie", m, extras),
  ]);
  const redraw = () => {
    // The file last played, else the first.
    const last = [...progress.values()].sort((a, b) => b.updated_at - a.updated_at)[0];
    const file = m.files.find((f) => f.id === last?.file_id) ?? m.files[0];
    const p = file ? progress.get(file.id) : null;
    const target = !file
      ? null
      : partway(p)
        ? { href: playHref(file.id, { movie: m.id }), label: `Resume · ${timeLeft(p)}` }
        : { href: playHref(file.id, { movie: m.id }, Boolean(p?.watched)), label: p?.watched ? "Play again" : runtime(m.runtime_minutes) ? `Play · ${runtime(m.runtime_minutes)}` : "Play" };
    aim(head.play, target, m.title);
    const watched = Boolean(p?.watched);
    mark.textContent = watched ? "Mark unwatched" : "Mark watched";
    mark.onclick = () => markFiles(m.files, !watched, progress, mark).then(redraw);
  };
  redraw();
  colour(m, head.play, gen);
  show(
    m.title,
    head.el,
    m.files.length ? null : h("p", { class: "status" }, "No file for this movie right now."),
    moreLinks("movie", m, extras, credits),
    moreLikeThis("movie", m.id, gen),
    aboutSection(m, m.files, { admin, kind: "movie" }),
  );
}

// Where Play starts for a series: the episode partway through, else the next one, else the
// first on disk; with the season it's in.
async function seriesTarget(s, continuing, nextUp) {
  const going = continuing.find((c) => c.series_id === s.id && c.season !== null);
  const next = nextUp.find((n) => n.series_id === s.id);
  const at = going ?? next;
  const item = at
    ? {
        kind: "series",
        id: s.id,
        episode: { file_id: at.file_id, season: at.season, episode: at.episode },
        progress: going ? { file_id: going.file_id, position_seconds: going.position_seconds, duration_seconds: going.duration_seconds, watched: false } : null,
      }
    : { kind: "series", id: s.id };
  const target = await playTarget(item);
  return { target, season: at?.season ?? null };
}

// Whether the series is anime, for where its genres lead: the anime list finds it by name.
async function isAnime(s) {
  const found = await quiet(api(`/series?anime=only&limit=50&q=${encodeURIComponent(s.title)}`), { items: [] });
  return found.items.some((i) => i.id === s.id);
}

export async function series(id, params, gen, admin = false) {
  loadingPage("Series");
  const [s, progress, extras, credits, continuing, nextUp] = await Promise.all([
    api(`/series/${id}`),
    progressFor("series", id),
    quiet(api(`/series/${id}/extras`), []),
    quiet(api(`/series/${id}/credits`), { cast: [], crew: [], collection: null }),
    quiet(api("/me/continue"), []),
    quiet(api("/me/next-up"), []),
  ]);
  const [{ target, season }, anime] = await Promise.all([seriesTarget(s, continuing, nextUp), isAnime(s)]);
  if (gen !== generation) return;
  const list = myListToggle();
  list.show({ kind: "series", id: s.id, title: s.title });
  const numbered = s.seasons.filter((x) => x.season > 0).length;
  const markAll = h("button", { type: "button", class: "quiet" }, "Mark all watched");
  const head = header(s, ["Series", s.year && String(s.year), numbered ? plural(numbered, "season", "seasons") : null, s.status], (g) => `#/${anime ? "anime" : "shows"}?genre=${encodeURIComponent(g)}`, [
    s.seasons.some((x) => x.available) ? markAll : null,
    list.el,
    trailerButton("series", s, extras),
  ]);
  aim(head.play, target, s.title);
  colour(s, head.play, gen);
  const asked = params.has("season") ? Number(params.get("season")) : null;
  const opening = s.seasons.some((x) => x.season === asked) ? asked : season ?? seasonOrder(s.seasons).find((x) => x.available)?.season;
  const picker = () => episodesPicker(s, progress, { opening: Number(new URLSearchParams(location.hash.split("?")[1] ?? "").get("season") ?? opening), gen, admin });
  let episodes = s.seasons.length ? picker() : h("p", { class: "status" }, "No seasons found for this series yet.");
  let all = false;
  markAll.addEventListener("click", async () => {
    if (!(await markSeries(s.id, null, !all, progress, markAll))) return;
    all = !all;
    markAll.textContent = all ? "Mark all unwatched" : "Mark all watched";
    if (gen !== generation) return;
    const fresh = picker();
    episodes.replaceWith(fresh);
    episodes = fresh;
  });
  show(s.title, head.el, episodes, moreLinks("series", s, extras, credits), moreLikeThis("series", s.id, gen), aboutSection(s, [], { admin, kind: "series", unparsed: s.unparsed_files }));
}
