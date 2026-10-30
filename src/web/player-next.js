// What floats over the picture, and the episodes around the one playing: Skip recap, intro,
// credits and preview while those parts play; links to the previous and next episode beside
// the player's options; and "Next episode in 10" over the picture once the credits start (or
// 20 seconds from the end when the episode has no credits marked), which starts the next one
// when it reaches nothing, with Play now and Cancel. Cancelling keeps it away for this episode.

import { announce, api, episodeLabel, h } from "/web/lib.js";
import { episodeCode } from "/web/watch-play.js";

const UP_NEXT_SECONDS = 10;
// Without credits marked, the card comes this long before the end.
const NO_CREDITS_SECONDS = 20;

// `where`: { seriesId, seasonNumber, episodeNumber, season, episode, title } for the episode
// playing; season and title are what the server said about them, null for a movie.

// The episode with a file just after (or before) this one, crossing into the next (or
// previous) season with one.
async function neighbour(where, later) {
  const { seriesId, seasonNumber, episodeNumber, season, episode, title } = where;
  if (!seriesId || !episode) return null;
  const near = (episodes, past) => {
    const found = episodes.filter((e) => e.files.length && (past === null || (later ? e.episode > past : e.episode < past)));
    return later ? found[0] : found.at(-1);
  };
  const here = near(season.episodes, episodeNumber);
  if (here) return { season: seasonNumber, episode: here };
  const seasons = (title?.seasons ?? []).filter((s) => (later ? s.season > seasonNumber : s.season < seasonNumber) && s.available > 0);
  const other = later ? seasons[0] : seasons.at(-1);
  if (!other) return null;
  const there = await api(`/series/${seriesId}/seasons/${other.season}`);
  const found = near(there.episodes, null);
  return found ? { season: other.season, episode: found } : null;
}

// A Skip button for the recap, the intro, the credits and the preview, each shown while its
// part plays (over the picture, so in full screen too); null when the episode has none of them.
export function skipButtons(video, segments) {
  const skip = h("div", { class: "skip-parts" });
  const skippable = [
    ["Skip recap", segments?.recap],
    ["Skip intro", segments?.intro],
    ["Skip credits", segments?.credits],
    ["Skip preview", segments?.preview],
  ].filter(([, part]) => part);
  for (const [label, part] of skippable) {
    const button = h("button", { type: "button", hidden: true }, label);
    button.addEventListener("click", () => {
      video.currentTime = part.end_seconds;
      button.hidden = true;
      video.focus();
    });
    // Offered from the part's start until a second before its end.
    video.addEventListener("timeupdate", () => {
      const at = video.currentTime;
      button.hidden = !(at >= part.start_seconds && at < part.end_seconds - 1);
    });
    skip.append(button);
  }
  return skippable.length ? skip : null;
}

// Where an episode found by `neighbour` plays.
export function episodeHref(seriesId, n) {
  return `#/play/${n.episode.files[0].id}?series=${seriesId}&season=${n.season}&episode=${n.episode.episode}`;
}

// Previous and next episode, as links beside the options (filled in once looked up). Returns
// the links and `after`, a promise of the next episode (null when there's none).
export function episodeLinks(where, ctx) {
  const nav = h("nav", { class: "episode-nav", "aria-label": "Episodes" });
  const quiet = (later) => neighbour(where, later).catch(() => null);
  const [before, after] = where.seriesId && where.episode ? [quiet(false), quiet(true)] : [Promise.resolve(null), Promise.resolve(null)];
  Promise.all([before, after]).then(([b, a]) => {
    if (!ctx.current()) return;
    // The DOM's replaceChildren writes a null as the text "null": the missing ones are left out.
    const links = [
      b && h("a", { class: "button quiet", href: episodeHref(where.seriesId, b), rel: "prev" }, `Previous: ${episodeLabel(b.season, b.episode.episode)}`),
      a && h("a", { class: "button quiet", href: episodeHref(where.seriesId, a), rel: "next" }, `Next: ${episodeLabel(a.season, a.episode.episode)}`),
    ];
    nav.replaceChildren(...links.filter(Boolean));
  });
  return { nav, after };
}

// The card over the picture offering the next episode, and the panel under it that says
// "Finished" when there's none. Returns { card, panel, cancel }: `cancel` stops a countdown on
// leaving.
export function upNextPanel(video, where, after, { ctx, report, backHref, heading, credits }) {
  const card = h("div", { class: "up-next-card", hidden: true, role: "group", "aria-label": "Up next" });
  const panel = h("div", { class: "up-next" });
  const s = { next: null, href: null, timer: null, left: UP_NEXT_SECONDS, dismissed: false, ctx };
  after.then((next) => {
    s.next = next;
    s.href = next ? episodeHref(where.seriesId, next) : null;
  });
  const startsAt = () => {
    const end = Number.isFinite(video.duration) ? video.duration : Infinity;
    return credits?.start_seconds ?? end - NO_CREDITS_SECONDS;
  };
  const stop = () => {
    clearInterval(s.timer);
    s.timer = null;
  };
  const hide = () => {
    stop();
    card.hidden = true;
  };
  video.addEventListener("timeupdate", () => {
    if (!s.next || s.dismissed) return;
    if (video.currentTime >= startsAt()) {
      if (card.hidden) offer(card, s, hide, false);
    } else if (!card.hidden) {
      // Gone back before the credits: put it away until they come round again.
      hide();
      s.left = UP_NEXT_SECONDS;
    }
  });
  video.addEventListener("ended", () => {
    report();
    if (!ctx.current()) return;
    if (s.next && !s.dismissed) {
      if (card.hidden) offer(card, s, hide, true);
      else card.querySelector("a")?.focus();
      return;
    }
    if (!s.next) panel.replaceChildren(h("p", {}, "Finished. ", h("a", { href: backHref }, `Back to ${heading}`)));
  });
  video.addEventListener("play", () => panel.replaceChildren());
  return { card, panel, cancel: stop };
}

// Fills the card with the next episode, Play now and Cancel, says so, and counts down while the
// video plays (or at once when it has ended). `ended` moves focus to Play now: the picture has
// stopped, so nothing is taken from someone watching.
function offer(card, s, hide, ended) {
  const { next } = s;
  const code = episodeCode(next.season, next.episode.episode);
  const seconds = h("span", {}, String(s.left));
  const playNow = h("a", { class: "button", href: s.href }, "Play now");
  const cancel = h("button", { type: "button", class: "quiet" }, "Cancel");
  cancel.addEventListener("click", () => {
    s.dismissed = true;
    hide();
    announce("The next episode won't start by itself.");
  });
  card.replaceChildren(
    h("p", { class: "up-next-label" }, h("span", { class: "visually-hidden" }, "Up next. "), "Next episode in ", seconds),
    h("p", { class: "up-next-title" }, [code, next.episode.name].filter(Boolean).join(" · ")),
    h("p", { class: "actions" }, playNow, cancel),
  );
  card.hidden = false;
  if (ended) playNow.focus();
  announce(`Up next: ${episodeLabel(next.season, next.episode.episode, next.episode.name)}. It starts in ${s.left} seconds.`);
  const video = card.closest(".stage")?.querySelector("video");
  s.timer = setInterval(() => {
    if (!s.ctx.current()) return hide();
    // Counts only while the picture moves, or once it has ended.
    if (video && video.paused && !video.ended) return;
    s.left -= 1;
    seconds.textContent = String(Math.max(s.left, 0));
    if (s.left <= 0) {
      hide();
      location.hash = s.href;
    }
  }, 1000);
}
