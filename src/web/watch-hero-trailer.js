// The hero's preview: after a few seconds on a pick, a muted preview plays behind the words,
// with Unmute and Stop beside the pick's buttons. The backdrop comes back when the preview
// ends, when it's stopped, or when the hero moves on.
//
// The preview is the pick's trailer when its folder has one (`/home`'s `trailer`), played as it
// is when this browser can play it, else converted. Without one, it's 40 seconds of the title
// itself, from where the viewer left it or an eighth of the way in (past most films' titles and
// most episodes' recaps). Conversions are 720p H.264 at 3 Mbit/s, small enough to start at once,
// and are asked for only on the home network, leaving the connection to whatever is being
// watched. Nothing about a preview counts as watching. Trailers online aren't used here: YouTube's
// player won't play without a referrer, which Melete's pages never send. Mneme does the same.
//
// Never when the device asks for less motion. On a phone-sized screen only when the user's
// playback preferences say every screen; "Never" there turns previews off.
//
// heroTrailer(art) → { el, show(item), stop(), playing() }: `art` is where the picture goes,
// `el` holds the buttons, `show` starts the wait for an item's preview, and `playing` says
// whether one is playing now, so the hero holds still meanwhile.

import { SignedOut, announce, api, h } from "/web/lib.js";
import { HLS_MODULE, choose, mse } from "/web/player-choice.js";
import { icon } from "/web/music-controls.js";
import { playTarget } from "/web/watch-play.js";

const WAIT_MS = 4000;
const CLIP_MS = 40_000;
// Enough for nearly any trailer to finish.
const TRAILER_MS = 150_000;
const CONVERSION = { max_height: 720, max_bitrate: 3_000_000, video_codec: "h264" };
const stillness = matchMedia("(prefers-reduced-motion: reduce)");
const phoneSized = matchMedia("(max-width: 40rem)");

let preferences = null;
const playbackPreferences = () => (preferences ??= api("/me/playback").catch(() => ({ hero_trailers: "large_screens" })));

function allowed(prefs) {
  if (stillness.matches || document.hidden || prefs.hero_trailers === "off") return false;
  return prefs.hero_trailers === "everywhere" || !phoneSized.matches;
}

// Whether this page came from the home network: a private address, a `.local` name or a name
// without dots. A guess, which is all it needs to be: it only decides whether to convert.
function onHomeNetwork(host = location.hostname) {
  const name = host.toLowerCase();
  if (name === "localhost" || name.endsWith(".local") || !name.includes(".")) return true;
  const parts = name.split(".").map(Number);
  if (parts.length !== 4 || parts.some((n) => !Number.isInteger(n))) return false;
  const [a, b] = parts;
  return a === 10 || a === 127 || (a === 192 && b === 168) || (a === 172 && b >= 16 && b <= 31);
}

// Where the viewer left the file, else an eighth of the way in.
function startOf(item, fileId, info) {
  const p = item.progress;
  if (p && !p.watched && p.position_seconds > 0 && p.file_id === fileId) return p.position_seconds;
  const length = item.kind === "movie" && item.runtime_minutes ? item.runtime_minutes * 60 : info.media?.duration_seconds;
  return (length ?? 600) / 8;
}

// What to play for `item`: { url, hls, at, ms, trailer }, or null for nothing.
async function find(item) {
  const home = onHomeNetwork();
  const trailer = item.trailer?.source === "folder" ? item.trailer.file_id : null;
  if (trailer) {
    const info = await api(`/files/${trailer}`);
    if (choose(info, {}).direct) {
      const play = await api(`/files/${trailer}/play`, { method: "POST" });
      return { url: play.stream_url, hls: false, at: 0, ms: TRAILER_MS, trailer: true };
    }
    if (home) return convert(trailer, 0, TRAILER_MS, true);
  }
  if (!home) return null;
  const target = await playTarget(item);
  if (!target?.file_id) return null;
  const info = await api(`/files/${target.file_id}`);
  return convert(target.file_id, startOf(item, target.file_id, info), CLIP_MS, false);
}

async function convert(fileId, at, ms, trailer) {
  const answer = await api(`/files/${fileId}/transcode`, { method: "POST", body: CONVERSION });
  return { url: answer.playlist_url, hls: true, at, ms, trailer };
}

// Plays a conversion's playlist in `video`: through hls.js where the browser has Media Source,
// else the browser's own HLS. The hls.js player, to destroy at the end; false when neither can.
async function attach(video, url, at) {
  if (mse) {
    const { default: Hls } = await import(HLS_MODULE);
    if (Hls.isSupported()) {
      const hls = new Hls({ enableWorker: false, startPosition: at });
      hls.on(Hls.Events.ERROR, (_, data) => data.fatal && video.dispatchEvent(new Event("error")));
      hls.loadSource(url);
      hls.attachMedia(video);
      return hls;
    }
  }
  if (!video.canPlayType("application/vnd.apple.mpegurl")) return false;
  video.src = url;
  if (at > 0) video.addEventListener("loadedmetadata", () => (video.currentTime = at), { once: true });
  return null;
}

export function heroTrailer(art) {
  const t = { item: null, timer: null, length: null, picture: null, hls: null, muted: true, stopped: new Set() };
  const unmute = h("button", { type: "button", class: "quiet hero-trailer-button" }, icon("volume"), "Unmute");
  const stopButton = h("button", { type: "button", class: "quiet hero-trailer-button" }, icon("stop"), "Stop trailer");
  const el = h("div", { class: "hero-trailer", hidden: true, role: "group", "aria-label": "Preview" }, unmute, stopButton);

  const end = () => {
    clearTimeout(t.timer);
    clearTimeout(t.length);
    t.timer = t.length = null;
    t.hls?.destroy();
    t.hls = null;
    t.picture?.removeAttribute("src");
    t.picture?.remove();
    t.picture = null;
    art.classList.remove("trailer-playing");
    el.hidden = true;
  };

  unmute.addEventListener("click", () => {
    t.muted = !t.muted;
    if (t.picture) t.picture.muted = t.muted;
    unmute.replaceChildren(icon(t.muted ? "volume" : "muted"), t.muted ? "Unmute" : "Mute");
  });
  stopButton.addEventListener("click", () => {
    if (t.item) t.stopped.add(`${t.item.kind}:${t.item.id}`);
    const focus = el.contains(document.activeElement);
    end();
    announce("Preview stopped.");
    if (focus) art.closest(".hero")?.querySelector(".hero-play")?.focus();
  });

  const begin = async (item, found) => {
    if (t.item !== item) return;
    t.muted = true;
    unmute.replaceChildren(icon("volume"), "Unmute");
    stopButton.replaceChildren(icon("stop"), found.trailer ? "Stop trailer" : "Stop clip");
    const video = h("video", { class: "hero-trailer-picture", muted: true, autoplay: true, playsinline: true, "aria-hidden": "true" });
    video.muted = true;
    video.addEventListener("ended", end);
    video.addEventListener("error", end);
    t.picture = video;
    if (found.hls) {
      const hls = await attach(video, found.url, found.at);
      if (hls === false || t.picture !== video) {
        hls?.destroy();
        if (t.picture === video) end();
        return;
      }
      t.hls = hls;
    } else {
      video.src = found.url;
    }
    t.length = setTimeout(end, found.ms);
    art.append(video);
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
        if (found) await begin(item, found);
      } catch (err) {
        // Without a preview to play, the backdrop stays; a session that ended says so.
        if (err instanceof SignedOut) announce("Your session ended. Sign in again.");
      }
    }, WAIT_MS);
  };
  addEventListener("visibilitychange", () => document.hidden && end());
  return { el, show, stop: end, playing: () => Boolean(t.picture) };
}
