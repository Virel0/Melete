// The hero's trailer: after a few seconds on a pick, its trailer plays muted behind the words,
// with Unmute and Stop trailer beside the pick's buttons. The backdrop comes back when the
// trailer ends, when it's stopped, or when the hero moves on.
//
// `/home` says where each pick's trailer comes from: one in the title's folder first, played as
// it is when this browser can play it (it's never converted for this); without one, and only
// while an admin has trailers online on, the one TMDB lists, in YouTube's privacy-enhanced
// player. Nothing is loaded from YouTube until that trailer starts, and the frame goes when it
// stops.
//
// Never when the device asks for less motion. On a phone-sized screen only when the user's
// playback preferences say every screen; "Never" there turns trailers off.
//
// heroTrailer(art) → { el, show(item), stop(), playing() }: `art` is where the picture goes,
// `el` holds the buttons, `show` starts the wait for an item's trailer, and `playing` says
// whether one is playing now, so the hero holds still meanwhile.

import { SignedOut, announce, api, h } from "/web/lib.js";
import { choose } from "/web/player-choice.js";
import { icon } from "/web/music-controls.js";
import { trailersOn } from "/web/title-trailer.js";

const WAIT_MS = 4000;
const PLAYER = "https://www.youtube-nocookie.com";
const stillness = matchMedia("(prefers-reduced-motion: reduce)");
const phoneSized = matchMedia("(max-width: 40rem)");

let preferences = null;
const playbackPreferences = () => (preferences ??= api("/me/playback").catch(() => ({ hero_trailers: "large_screens" })));

function allowed(prefs) {
  if (stillness.matches || document.hidden || prefs.hero_trailers === "off") return false;
  return prefs.hero_trailers === "everywhere" || !phoneSized.matches;
}

// The trailer to play for `item`, from where `/home` says it comes (`item.trailer`): { video }
// for one in its folder this browser plays as it is, { embed } for one online, or null. A folder
// trailer the browser can't play as it is gives way to the online one, when there is one.
async function find(item) {
  const trailer = item.trailer;
  if (!trailer) return null;
  if (trailer.source === "online") return online(trailer.embed_url, item.title);
  const [info, play] = await Promise.all([api(`/files/${trailer.file_id}`), api(`/files/${trailer.file_id}/play`, { method: "POST" })]);
  if (choose(info, {}).direct) return { video: play.stream_url };
  if (!(await trailersOn())) return null;
  const plural = item.kind === "movie" ? "movies" : "series";
  const answer = await api(`/${plural}/${item.id}/trailer`).catch((err) => {
    if (err instanceof SignedOut) throw err;
    return null;
  });
  return answer?.site === "youtube" ? online(answer.embed_url, answer.name) : null;
}

// Only YouTube's privacy-enhanced player, whatever the answer says.
function online(embedUrl, name) {
  return typeof embedUrl === "string" && embedUrl.startsWith(`${PLAYER}/embed/`) ? { embed: embedUrl, name } : null;
}

export function heroTrailer(art) {
  const t = { item: null, timer: null, picture: null, muted: true, stopped: new Set() };
  const unmute = h("button", { type: "button", class: "quiet hero-trailer-button" }, icon("volume"), "Unmute");
  const stopButton = h("button", { type: "button", class: "quiet hero-trailer-button" }, icon("stop"), "Stop trailer");
  const el = h("div", { class: "hero-trailer", hidden: true, role: "group", "aria-label": "Trailer" }, unmute, stopButton);

  const end = () => {
    clearTimeout(t.timer);
    t.timer = null;
    t.picture?.remove();
    t.picture = null;
    art.classList.remove("trailer-playing");
    el.hidden = true;
    removeEventListener("message", heard);
  };
  // YouTube's player says when the trailer ends (state 0), once asked to tell.
  const heard = (event) => {
    if (event.origin !== PLAYER || event.source !== t.picture?.contentWindow) return;
    let said;
    try {
      said = JSON.parse(event.data);
    } catch {
      return;
    }
    if (said?.event === "onStateChange" && said.info === 0) end();
  };
  const tellYoutube = (func) =>
    t.picture?.contentWindow?.postMessage(JSON.stringify({ event: "command", func, args: [] }), PLAYER);

  unmute.addEventListener("click", () => {
    t.muted = !t.muted;
    if (t.picture instanceof HTMLVideoElement) t.picture.muted = t.muted;
    else tellYoutube(t.muted ? "mute" : "unMute");
    unmute.replaceChildren(icon(t.muted ? "volume" : "muted"), t.muted ? "Unmute" : "Mute");
  });
  stopButton.addEventListener("click", () => {
    if (t.item) t.stopped.add(`${t.item.kind}:${t.item.id}`);
    const focus = el.contains(document.activeElement);
    end();
    announce("Trailer stopped.");
    if (focus) art.closest(".hero")?.querySelector(".hero-play")?.focus();
  });

  const begin = (item, found) => {
    if (t.item !== item) return;
    t.muted = true;
    unmute.replaceChildren(icon("volume"), "Unmute");
    if (found.video) {
      const video = h("video", { class: "hero-trailer-picture", muted: true, autoplay: true, playsinline: true, "aria-hidden": "true" });
      video.muted = true;
      video.addEventListener("ended", end);
      video.addEventListener("error", end);
      video.src = found.video;
      t.picture = video;
    } else {
      const src = `${found.embed}?autoplay=1&mute=1&controls=0&rel=0&playsinline=1&enablejsapi=1&origin=${encodeURIComponent(location.origin)}`;
      const frame = h("iframe", { class: "hero-trailer-picture", src, title: found.name || `Trailer: ${item.title}`, allow: "autoplay; encrypted-media", tabindex: "-1" });
      frame.addEventListener("load", () => frame.contentWindow?.postMessage(JSON.stringify({ event: "listening" }), PLAYER));
      addEventListener("message", heard);
      t.picture = frame;
    }
    art.append(t.picture);
    art.classList.add("trailer-playing");
    el.hidden = false;
  };

  const show = (item) => {
    end();
    t.item = item;
    if (!item || t.stopped.has(`${item.kind}:${item.id}`)) return;
    t.timer = setTimeout(async () => {
      try {
        if (t.item !== item || !allowed(await playbackPreferences())) return;
        const found = await find(item);
        if (found) begin(item, found);
      } catch (err) {
        // Without a trailer to play, the backdrop stays; a session that ended says so.
        if (err instanceof SignedOut) announce("Your session ended. Sign in again.");
      }
    }, WAIT_MS);
  };
  addEventListener("visibilitychange", () => document.hidden && end());
  return { el, show, stop: end, playing: () => Boolean(t.picture) };
}
