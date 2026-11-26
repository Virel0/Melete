// Watch Together: this device in a watch party (`/watch-parties*`), played in step with the
// others in it. The server keeps the party (which file, where and whether it plays, at a
// version); each change here is sent against the version last seen, and every device hears of
// each change from the event stream's `watch_party` event and follows it. A change refused
// because another device got in first is dropped, and the newer state followed instead.
//
// Each device plays its own stream, with its own audio and subtitles. Anyone in the party plays,
// pauses and seeks; the host alone moves it to another file: a host who opens another episode
// takes the party there, and everyone else's player follows.
//
// - attachParty(s, where): the player's part, while it's open: follows the party when it plays
//   this file, sends this player's play, pause and seek; returns the Together panel's body.
// - togetherPage(ctx): `#/together`, to join by code or go back to the party.

import { SignedOut, announce, api, h } from "/web/lib.js";

// Off by more than this, in seconds, a player seeks to where the party is.
const DRIFT = 1.5;
// Changes close together go as one.
const SEND_AFTER_MS = 300;
// How long a followed change's own play, pause and seek events are left unsent.
const SETTLE_MS = 800;
const RETRY_MS = 15_000;
// The caps a host may hold guests' streams to, in bits a second; none is the first.
const CAPS = [
  [null, "As everyone's"],
  [4_000_000, "4 Mbps (720p)"],
  [8_000_000, "8 Mbps (1080p)"],
  [20_000_000, "20 Mbps (4K)"],
];

let party = null;
let me = null;
let skew = 0;
let source = null;
let retry = null;
let following = 0;
let sendTimer = null;
// The open player: its state and the file it plays.
let player = null;
const watchers = new Set();

const changed = () => watchers.forEach((fn) => fn());

// The player page for the party's file.
export function partyHref(p) {
  return p.movie_id ? `#/play/${p.file_id}?movie=${p.movie_id}` : `#/play/${p.file_id}?series=${p.series_id}&season=${p.season}&episode=${p.episode}`;
}

const hostHere = () => Boolean(party && me && party.host === me.username);
const onParty = () => Boolean(party && player && Number(player.fileId) === party.file_id);

// An answer with a party and the server's clock.
function took(answer) {
  skew = answer.server_time - Date.now();
  party = answer.party;
  listen();
  changed();
}

function ended(words) {
  party = null;
  clearTimeout(retry);
  clearTimeout(sendTimer);
  source?.close();
  source = null;
  announce(words);
  changed();
}

// Where the party's file is now, in seconds.
function placeOf(p) {
  return (p.position_ms + (p.playing ? Math.max(0, Date.now() + skew - p.position_at) : 0)) / 1000;
}

// This player's state, as the server takes it.
function here() {
  const video = player.s.video;
  return {
    file_id: Number(player.fileId),
    position_ms: Math.round((Number.isFinite(video.currentTime) ? video.currentTime : 0) * 1000),
    playing: !video.paused,
  };
}

// Plays the party where it plays; another file is gone to.
function follow(p) {
  if (!player) return;
  if (Number(player.fileId) !== p.file_id) {
    location.hash = partyHref(p);
    return;
  }
  const video = player.s.video;
  following++;
  try {
    const at = placeOf(p);
    if (Math.abs(video.currentTime - at) > DRIFT) video.currentTime = at;
    if (p.playing && video.paused) video.play().catch(() => {});
    if (!p.playing && !video.paused) video.pause();
  } finally {
    setTimeout(() => following--, SETTLE_MS);
  }
}

async function refresh() {
  if (!party) return;
  try {
    took(await api(`/watch-parties/${party.id}`));
    follow(party);
  } catch (err) {
    if (err.status === 404) ended("The watch party has ended.");
  }
}

async function send(body = here()) {
  if (!party) return;
  try {
    took(await api(`/watch-parties/${party.id}`, { method: "PUT", body: { version: party.version, ...body } }));
  } catch (err) {
    if (err.status === 409) return refresh();
    if (err.status === 404) return ended("The watch party has ended.");
    if (err instanceof SignedOut) return;
    announce(`Couldn't change the watch party: ${err.message}`);
  }
}

// Play, pause or a seek here: sent, unless it's this player following.
function local() {
  if (!onParty() || following) return;
  clearTimeout(sendTimer);
  sendTimer = setTimeout(send, SEND_AFTER_MS);
}

// The event stream, while in a party: a newer version is followed; its absence means it ended.
function listen() {
  if (source || !party) return;
  source = new EventSource("/api/v1/events");
  source.addEventListener("watch_party", (message) => {
    let parties;
    try {
      parties = JSON.parse(message.data);
    } catch {
      return;
    }
    if (!party) return;
    const now = parties.find((p) => p.id === party.id);
    if (!now) return ended("The watch party has ended.");
    const newer = now.version > party.version;
    if (now.version >= party.version) party = now;
    changed();
    if (newer && player) follow(now);
  });
  source.addEventListener("error", () => {
    if (source?.readyState !== EventSource.CLOSED) return;
    source = null;
    retry = setTimeout(listen, RETRY_MS);
  });
}

// The party this device is still in, after a reload.
async function pickUp() {
  if (party) return;
  try {
    const mine = await api("/watch-parties");
    if (mine[0] && !party) {
      party = mine[0];
      await refresh();
    }
  } catch {
    // Starting or joining still works.
  }
}

export function attachParty(s, ctx) {
  me = ctx.me ?? me;
  player = { s, fileId: s.fileId };
  const video = s.video;
  const events = ["play", "pause", "seeked"];
  events.forEach((e) => video.addEventListener(e, local));
  const body = h("div", { class: "together" });
  const draw = () => body.replaceChildren(...panelBody(ctx));
  watchers.add(draw);
  draw();
  // Once the video can play: follow the party here, or, as its host, take it to this file.
  video.addEventListener(
    "canplay",
    async () => {
      await pickUp();
      if (!party || player?.s !== s) return;
      if (onParty()) follow(party);
      else if (hostHere()) send({ file_id: Number(s.fileId), position_ms: 0, playing: !video.paused });
      changed();
    },
    { once: true },
  );
  return {
    body,
    stop() {
      events.forEach((e) => video.removeEventListener(e, local));
      watchers.delete(draw);
      if (player?.s === s) player = null;
    },
  };
}

function failed(err, said, ctx) {
  if (err instanceof SignedOut) return ctx.signedOut();
  said.textContent = err.message;
}

// The Together panel: start or join; or the party's code, who's in it, leave and end.
function panelBody(ctx) {
  if (!party) {
    const cap = h("select", { id: "party-guest-cap" }, CAPS.map(([bits, label], i) => h("option", { value: String(i) }, label)));
    const said = h("p", { class: "note", role: "status" });
    const start = h("button", { type: "button" }, "Start a watch party");
    start.addEventListener("click", async () => {
      start.disabled = true;
      try {
        took(await api("/watch-parties", { method: "POST", body: { ...here(), guest_max_bitrate: CAPS[Number(cap.value)][0] } }));
        announce(`Watch party started. Its code is ${party.code.split("").join(" ")}.`);
      } catch (err) {
        failed(err, said, ctx);
        start.disabled = false;
      }
    });
    return [
      h("p", { class: "note" }, "Watch this with others in step: anyone in it can play, pause and seek, and each keeps their own audio and subtitles."),
      h("div", { class: "field" }, h("label", { for: cap.id }, "Guests' quality"), cap),
      start,
      said,
      joinForm(ctx),
    ];
  }
  const members = party.members.map((m) => h("li", {}, m.username, m.you ? " (you)" : "", m.username === party.host ? " · host" : ""));
  const leave = h("button", { type: "button", class: "quiet" }, "Leave");
  leave.addEventListener("click", async () => {
    await api(`/watch-parties/${party.id}/leave`, { method: "POST" }).catch(() => {});
    ended("You left the watch party. The video goes on here.");
  });
  const end = hostHere() || me?.is_admin ? h("button", { type: "button", class: "quiet" }, "End for everyone") : null;
  end?.addEventListener("click", async () => {
    await api(`/watch-parties/${party.id}`, { method: "DELETE" }).catch(() => {});
    ended("You ended the watch party for everyone.");
  });
  return [
    h("p", {}, "Code: ", h("strong", { class: "party-code" }, party.code)),
    h("p", { class: "note" }, onParty() ? "Others join with this code under Watch together." : h("a", { href: partyHref(party) }, "Back to what the party is watching")),
    h("ul", { class: "plain" }, members),
    h("div", { class: "form-actions" }, leave, end),
  ];
}

// Joining with a code: the party's file opens and plays where the party is.
function joinForm(ctx) {
  const input = h("input", { id: "party-code", autocomplete: "off", spellcheck: "false", maxlength: "9", placeholder: "ABC123" });
  const said = h("p", { class: "note", role: "status" });
  const form = h("form", { class: "search-row", novalidate: true }, h("label", { for: input.id }, "Join with a code"), input, h("button", { type: "submit", class: "quiet" }, "Join"));
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    try {
      took(await api("/watch-parties/join", { method: "POST", body: { code: input.value } }));
      announce(`Joined ${party.host}'s watch party`);
      location.hash = partyHref(party);
    } catch (err) {
      failed(err, said, ctx);
    }
  });
  return h("div", {}, form, said);
}

// `#/together`: join by code, or go back to the party this device is in.
export async function togetherPage(ctx, show) {
  me = ctx.me ?? me;
  await pickUp();
  if (!ctx.current()) return;
  const back = party ? h("p", {}, `You're in ${party.host}'s watch party (code ${party.code}). `, h("a", { href: partyHref(party) }, "Go back to it")) : null;
  show(
    "Watch together",
    h("h1", {}, "Watch together"),
    h("p", { class: "note" }, "Start a watch party from the player's Together button while something plays. To join one, type the code whoever started it gives you."),
    back,
    joinForm(ctx),
  );
}
