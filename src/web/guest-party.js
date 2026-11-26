// A watch party guest's page: someone with no account, in by an invite link
// (`#/guest/mpi_…`). They give a name, and the link's secret leaves the address bar at once.
// Then a small player follows the party (`/api/v1/party*`): it plays the file the party plays,
// where and as it plays, and sends their own play, pause and seek. They choose their own audio
// and subtitles. Loading or buffering, it says so, and the party waits for them a while.
//
// It plays the file as it is when the party holds guests to no cap and the file's own audio is
// chosen; otherwise it asks for a conversion (the picture copied when there's no cap).

import { SignedOut, announce, api, h, show } from "/web/lib.js";
import { HLS_MODULE } from "/web/player-choice.js";

// Off by more than this, in seconds, the player seeks to where the party is.
const DRIFT = 1.5;
const SEND_AFTER_MS = 300;
const SETTLE_MS = 800;
const WAIT_AFTER_MS = 700;

const g = {
  party: null,
  skew: 0,
  video: null,
  hls: null,
  source: null,
  file: null,
  audio: null,
  following: 0,
  sendTimer: null,
  waitTimer: null,
  waitingSaid: false,
  note: null,
  side: null,
};

const ENDED = "This watch party has ended, or your place in it has. Ask whoever invited you for a new link.";

// The guest's page: the join form for an invite in the address, or the party they're in.
export async function guestPage() {
  const invite = location.hash.match(/^#\/guest\/(mpi_[0-9a-f]+)$/)?.[1];
  if (invite) return joinForm(invite);
  try {
    enter(await api("/party"));
  } catch (err) {
    over(err instanceof SignedOut ? ENDED : `Couldn't open the watch party: ${err.message}`);
  }
}

function over(words) {
  stopAll();
  show("Watch party", h("h1", {}, "Watch party"), h("p", { class: "status", role: "status" }, words));
}

function joinForm(invite) {
  const name = h("input", { id: "guest-name", autocomplete: "nickname", maxlength: "40", required: true });
  const said = h("p", { class: "field-error", role: "alert" });
  const button = h("button", { type: "submit" }, "Join");
  const form = h(
    "form",
    { novalidate: true },
    h("div", { class: "field" }, h("label", { for: name.id }, "Your name"), h("p", { class: "note" }, "Shown to the others in the party."), name),
    said,
    h("div", { class: "form-actions" }, button),
  );
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    if (!name.value.trim()) {
      said.textContent = "Type the name to show the others.";
      name.focus();
      return;
    }
    button.disabled = true;
    try {
      const joined = await api("/party/join", { method: "POST", body: { invite, name: name.value } });
      // The link's secret is spent; it leaves the address bar and the history.
      history.replaceState(null, "", "#/guest");
      enter(joined);
    } catch (err) {
      said.textContent = err.message;
      button.disabled = false;
    }
  });
  show(
    "Join a watch party",
    h("h1", {}, "Join a watch party"),
    h("p", {}, "You've been invited to watch something together. Everyone in the party can play, pause and skip, and you choose your own audio and subtitles."),
    form,
  );
  name.focus();
}

// Where the party's file is now, in seconds.
const placeOf = (p) => (p.position_ms + (p.playing ? Math.max(0, Date.now() + g.skew - p.position_at) : 0)) / 1000;

function took(answer) {
  g.skew = answer.server_time - Date.now();
  g.party = answer.party;
  draw();
}

// The party's page: what plays, who's waited for, the player, the choices, who's in it.
function enter(answer) {
  stopAll();
  g.video = h("video", { controls: true, playsinline: true, preload: "auto", class: "guest-video" });
  g.note = h("p", { class: "status", role: "status" });
  g.side = h("div", { class: "guest-side" });
  g.audio = null;
  g.file = null;
  took(answer);
  for (const e of ["play", "pause", "seeked"]) g.video.addEventListener(e, () => local(e));
  g.video.addEventListener("waiting", stalled);
  g.video.addEventListener("canplay", ready);
  g.video.addEventListener("playing", ready);
  show("Watch party", h("h1", { class: "guest-title" }, g.party.title), h("div", { class: "stage guest-stage" }, g.video), g.note, g.side);
  load().then(listen);
}

// Plays the file the party plays: as it is, or a conversion; then where the party is.
async function load() {
  const p = g.party;
  try {
    g.file = await api("/party/file");
    const tracks = await api("/party/subtitles").catch(() => []);
    stalled();
    const own = g.audio === null || g.audio === defaultAudio(g.file);
    let base;
    if (!g.file.max_bitrate && own) {
      base = (await api("/party/play", { method: "POST" })).stream_url;
      unHls();
      g.video.src = base;
    } else {
      const ask = { ...(g.audio === null ? {} : { audio_stream: g.audio }) };
      let made;
      try {
        made = await api("/party/transcode", { method: "POST", body: g.file.max_bitrate ? ask : { ...ask, video_codec: "copy" } });
      } catch (err) {
        if (err.status !== 422) throw err;
        made = await api("/party/transcode", { method: "POST", body: ask });
      }
      base = made.playlist_url.replace(/^\/api\/v1\/hls\/([^/]+)\/.*$/, "/api/v1/stream/$1");
      await playHls(made.playlist_url);
    }
    textTracks(base, tracks);
    draw();
    g.video.addEventListener("loadedmetadata", () => follow(g.party), { once: true });
    if (p !== g.party) follow(g.party);
  } catch (err) {
    if (err instanceof SignedOut) return over(ENDED);
    g.note.textContent = `Couldn't play it: ${err.message}`;
  }
}

function unHls() {
  g.hls?.destroy();
  g.hls = null;
}

async function playHls(url) {
  unHls();
  if (g.video.canPlayType("application/vnd.apple.mpegurl")) {
    g.video.src = url;
    return;
  }
  const { default: Hls } = await import(HLS_MODULE);
  g.hls = new Hls({ enableWorker: false, startPosition: placeOf(g.party) });
  g.hls.loadSource(url);
  g.hls.attachMedia(g.video);
}

function defaultAudio(file) {
  const audio = file.media?.audio ?? [];
  return (audio.find((a) => a.default) ?? audio[0])?.index ?? null;
}

// The file's text subtitles, from the guest's own stream link; none showing until chosen.
function textTracks(base, tracks) {
  for (const t of [...g.video.querySelectorAll("track")]) t.remove();
  for (const t of tracks.filter((t) => t.text)) {
    g.video.append(h("track", { kind: "subtitles", src: `${base}/subtitles/${t.track}.vtt`, label: trackLabel(t), srclang: /^[a-z]{2,3}$/.test(t.language ?? "") ? t.language : null }));
  }
}

const trackLabel = (t) => [t.title, t.language, t.forced ? "forced" : null].filter(Boolean).join(" · ") || t.track;

// Plays the party where it plays.
function follow(p) {
  if (!g.video || !p) return;
  g.following++;
  try {
    const at = placeOf(p);
    if (Number.isFinite(g.video.duration) && Math.abs(g.video.currentTime - at) > DRIFT) g.video.currentTime = at;
    if (p.playing && g.video.paused) g.video.play().catch(() => (g.note.textContent = "Press play to join in."));
    if (!p.playing && !g.video.paused) g.video.pause();
  } finally {
    setTimeout(() => g.following--, SETTLE_MS);
  }
}

async function send() {
  const v = g.video;
  try {
    took(await api("/party", { method: "PUT", body: { version: g.party.version, position_ms: Math.round((v.currentTime || 0) * 1000), playing: !v.paused } }));
  } catch (err) {
    if (err instanceof SignedOut) return over(ENDED);
    if (err.status === 409) took(await api("/party").catch(() => ({ party: g.party, server_time: Date.now() + g.skew })));
    follow(g.party);
  }
}

// Play, pause or a seek here: sent, unless it's following. Play while the party plays (after a
// reload, say) catches up with it rather than sending this player's place.
function local(kind) {
  if (!g.party || g.following) return;
  if (kind === "play" && g.party.playing) return follow(g.party);
  clearTimeout(g.sendTimer);
  g.sendTimer = setTimeout(send, SEND_AFTER_MS);
}

async function sayWaiting(waiting) {
  if (!g.party || waiting === g.waitingSaid) return;
  g.waitingSaid = waiting;
  try {
    took(await api("/party/waiting", { method: "PUT", body: { waiting } }));
  } catch (err) {
    if (err instanceof SignedOut) over(ENDED);
  }
}

function stalled() {
  if (!g.party || !(g.party.playing || g.party.held)) return;
  clearTimeout(g.waitTimer);
  g.waitTimer = setTimeout(() => sayWaiting(true), WAIT_AFTER_MS);
}

function ready() {
  clearTimeout(g.waitTimer);
  if (g.waitingSaid) sayWaiting(false);
}

// The party as it changes: a newer version followed, another file loaded, its end said.
function listen() {
  g.source?.close();
  g.source = new EventSource("/api/v1/party/events");
  g.source.addEventListener("watch_party", (message) => {
    let answer;
    try {
      answer = JSON.parse(message.data);
    } catch {
      return;
    }
    if (!answer) return over(ENDED);
    const before = g.party;
    const newer = answer.party.version > before.version;
    if (answer.party.version >= before.version) took(answer);
    if (answer.party.file_id !== before.file_id) {
      document.querySelector(".guest-title").textContent = g.party.title;
      g.audio = null;
      return load();
    }
    if (newer) follow(g.party);
  });
}

function stopAll() {
  g.source?.close();
  g.source = null;
  unHls();
  clearTimeout(g.sendTimer);
  clearTimeout(g.waitTimer);
  g.waitingSaid = false;
}

// Who's waited for, the choices, who's in it, and Leave.
function draw() {
  if (!g.side || !g.party) return;
  const p = g.party;
  g.note.textContent = p.waiting?.length ? `Waiting for ${p.waiting.join(", ")}…` : "";
  const audio = g.file?.media?.audio ?? [];
  const pick = audio.length > 1 ? h("select", { id: "guest-audio" }, audio.map((a) => h("option", { value: String(a.index), selected: a.index === (g.audio ?? defaultAudio(g.file)) }, [a.title, a.language, a.channels ? `${a.channels} ch` : null].filter(Boolean).join(" · ") || `Track ${a.index}`))) : null;
  pick?.addEventListener("change", () => {
    g.audio = Number(pick.value);
    load();
  });
  const subs = [...(g.video?.textTracks ?? [])];
  const sub = subs.length ? h("select", { id: "guest-subtitles" }, h("option", { value: "" }, "Off"), subs.map((t, i) => h("option", { value: String(i), selected: t.mode === "showing" }, t.label))) : null;
  sub?.addEventListener("change", () => subs.forEach((t, i) => (t.mode = String(i) === sub.value ? "showing" : "disabled")));
  const leave = h("button", { type: "button", class: "quiet" }, "Leave the party");
  leave.addEventListener("click", async () => {
    await api("/party/leave", { method: "POST" }).catch(() => {});
    over("You left the watch party.");
    announce("You left the watch party.");
  });
  const people = [`${p.household} from the household`, ...p.guests.map((x) => (x.you ? `${x.name} (you)` : x.name))];
  const parts = [
    pick ? h("div", { class: "field" }, h("label", { for: pick.id }, "Audio"), pick) : null,
    sub ? h("div", { class: "field" }, h("label", { for: sub.id }, "Subtitles"), sub) : null,
    p.max_bitrate ? h("p", { class: "note" }, `Your stream is held to ${Math.round(p.max_bitrate / 1_000_000)} Mbps by whoever started the party.`) : null,
    h("p", { class: "note" }, "In the party: ", people.join(", ")),
    leave,
  ];
  g.side.replaceChildren(...parts.filter(Boolean));
}
