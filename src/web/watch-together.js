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
//
// Waiting for everyone: a player loading or buffering on the party's file says so, and a
// playing party pauses for it, playing on once everyone's ready (or after 30 seconds).

import { SignedOut, announce, api, h } from "/web/lib.js";
import { correct, partyClock, startsIn, timed } from "/web/party-clock.js";
import { tunnelInvite } from "/web/party-tunnel.js";

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
const clock = partyClock();
let startTimer = null;
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
  party = answer.party;
  listen();
  changed();
}

function ended(words) {
  party = null;
  waitingSaid = false;
  clearTimeout(waitTimer);
  clearTimeout(retry);
  clearTimeout(sendTimer);
  source?.close();
  source = null;
  announce(words);
  changed();
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
    clearTimeout(startTimer);
    if (correct(video, p, clock, Boolean(player.s.hls)) === "wait") {
      // A start set a moment ahead: played when it comes, with everyone.
      startTimer = setTimeout(() => party && follow(party), startsIn(p, clock));
    } else if (p.playing && video.paused) {
      video.play().catch(() => (player.s.status.textContent = "Press play to join in."));
    }
    if (!p.playing && !video.paused) video.pause();
  } finally {
    setTimeout(() => following--, SETTLE_MS);
  }
}

// Each second while the party plays here: the player kept to it, gently (see party-clock.js);
// a seek of its own isn't sent as this viewer's. Each minute, the clock asked afresh.
let ticks = 0;
function keepInStep() {
  ticks++;
  if (!onParty()) return;
  if (ticks % 60 === 0) timed(clock, () => api(`/watch-parties/${party.id}`)).catch(() => {});
  const video = player.s.video;
  if (!party.playing || video.paused || following) return;
  if (correct(video, party, clock, Boolean(player.s.hls)) === "seek") {
    following++;
    setTimeout(() => following--, SETTLE_MS);
  }
}

async function refresh() {
  if (!party) return;
  try {
    took(await timed(clock, () => api(`/watch-parties/${party.id}`)));
    follow(party);
  } catch (err) {
    if (err.status === 404) ended("The watch party has ended.");
  }
}

async function send(body = here()) {
  if (!party) return;
  try {
    took(await timed(clock, () => api(`/watch-parties/${party.id}`, { method: "PUT", body: { version: party.version, ...body } })));
  } catch (err) {
    if (err.status === 409) return refresh();
    if (err.status === 404) return ended("The watch party has ended.");
    if (err instanceof SignedOut) return;
    announce(`Couldn't change the watch party: ${err.message}`);
  }
}

// Whether this player has said it's waiting, and the wait about to be said.
let waitingSaid = false;
let waitTimer = null;
// A stall this short isn't worth stopping everyone for.
const WAIT_AFTER_MS = 700;

async function sayWaiting(waiting) {
  if (!party || waiting === waitingSaid) return;
  waitingSaid = waiting;
  try {
    took(await timed(clock, () => api(`/watch-parties/${party.id}/waiting`, { method: "PUT", body: { waiting } })));
  } catch (err) {
    if (err.status === 404) ended("The watch party has ended.");
  }
}

// The player stalls, or opens before it can play: waited for, if the party plays (or already
// waits). Ready again: no longer.
function stalled() {
  if (!onParty() || !(party.playing || party.held)) return;
  clearTimeout(waitTimer);
  waitTimer = setTimeout(() => sayWaiting(true), WAIT_AFTER_MS);
}
function ready() {
  clearTimeout(waitTimer);
  if (waitingSaid) sayWaiting(false);
}

// Play, pause or a seek here: sent, unless it's this player following.
function local(event) {
  if (!onParty() || following) return;
  // Play while the party plays (the browser stopped it playing by itself, say) catches up.
  if (event?.type === "play" && party.playing) {
    player.s.status.textContent = "";
    return follow(party);
  }
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
      // A few samples to start from; the shortest round trip wins.
      for (let i = 0; i < 3; i++) await timed(clock, () => api(`/watch-parties/${party.id}`)).catch(() => {});
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
  video.addEventListener("waiting", stalled);
  video.addEventListener("canplay", ready);
  video.addEventListener("playing", ready);
  const body = h("div", { class: "together" });
  const loop = setInterval(keepInStep, 1000);
  const draw = () => body.replaceChildren(...panelBody(ctx).filter(Boolean));
  watchers.add(draw);
  draw();
  pickUp().then(() => {
    if (player?.s === s && onParty() && video.readyState < 3) stalled();
  });
  // Once the video can play: follow the party here, or, as its host, take it to this file.
  video.addEventListener(
    "canplay",
    async () => {
      await pickUp();
      if (!party || player?.s !== s) return;
      ready();
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
      video.removeEventListener("waiting", stalled);
      video.removeEventListener("canplay", ready);
      video.removeEventListener("playing", ready);
      ready();
      clearInterval(loop);
      clearTimeout(startTimer);
      if (video.playbackRate !== 1) video.playbackRate = 1;
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
        took(await timed(clock, () => api("/watch-parties", { method: "POST", body: { ...here(), guest_max_bitrate: CAPS[Number(cap.value)][0] } })));
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
  const held = party.waiting?.length ? h("p", { class: "status", role: "status" }, `Waiting for ${party.waiting.join(", ")}…`) : null;
  return [
    held,
    h("p", {}, "Code: ", h("strong", { class: "party-code" }, party.code)),
    guestsPart(ctx),
    h("p", { class: "note" }, onParty() ? "Others join with this code under Watch together." : h("a", { href: partyHref(party) }, "Back to what the party is watching")),
    h("ul", { class: "plain" }, members),
    h("div", { class: "form-actions" }, leave, end),
  ];
}

// The invite link made last, kept while the panel is drawn again for the party's changes.
let invited = null;

// Guests with no account: who's in, Send away for each, and a link to let one more in.
function guestsPart(ctx) {
  const said = h("p", { class: "note", role: "status" });
  const link = h("div", { class: "invite-link" });
  const showLink = () => {
    if (invited?.party !== party.id) return;
    const field = h("input", { id: "invite-url", readonly: true, value: invited.url, "aria-label": "Invite link" });
    const copy = h("button", { type: "button", class: "quiet" }, "Copy");
    copy.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(invited.url);
        said.textContent = "Copied. It lets one person in, once, within a day.";
      } catch {
        field.select();
        said.textContent = "Select the link and copy it. It lets one person in, once, within a day.";
      }
    });
    link.replaceChildren(field, copy);
    said.textContent = invited.note;
  };
  const invite = h("button", { type: "button", class: "quiet" }, invited?.party === party.id ? "Another invite" : "Invite a guest");
  invite.addEventListener("click", async () => {
    invite.disabled = true;
    try {
      const made = await api(`/watch-parties/${party.id}/invites`, { method: "POST" });
      invited = {
        party: party.id,
        url: made.link ?? `${location.origin}${made.path}`,
        note: made.link
          ? "Send this to one person. It lets them in once, within a day, to this party alone."
          : "Send this to one person: it lets them in once, within a day. It begins with this page's address; set the address guests reach Melete at under Outside services, Watch party guests.",
      };
      showLink();
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      said.textContent = err.message;
    } finally {
      invite.disabled = false;
    }
  });
  const guests = (party.guests ?? []).map((g) => {
    const away = h("button", { type: "button", class: "quiet", "aria-label": `Send ${g.name} away` }, "Send away");
    away.addEventListener("click", async () => {
      away.disabled = true;
      try {
        await api(`/watch-parties/${party.id}/guests/${g.id}`, { method: "DELETE" });
        announce(`${g.name} was sent away.`);
        refresh();
      } catch (err) {
        if (err instanceof SignedOut) return ctx.signedOut();
        said.textContent = err.message;
        away.disabled = false;
      }
    });
    return h("li", {}, `${g.name} (guest) `, away);
  });
  showLink();
  // What a guest sees: an invite of its own, opened in a new tab as a guest would (a guest's
  // pass is apart from this sign-in, so it works in this browser).
  const preview = h("button", { type: "button", class: "quiet" }, "Open as a guest");
  preview.addEventListener("click", async () => {
    preview.disabled = true;
    try {
      const made = await api(`/watch-parties/${party.id}/invites`, { method: "POST" });
      window.open(`${location.origin}${made.path}`, "_blank", "noopener");
      said.textContent = "Opened as a guest in a new tab; it joins the party under the name you give. Send it away from here when you're done.";
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      said.textContent = err.message;
    } finally {
      preview.disabled = false;
    }
  });
  const tunnel = tunnelInvite(party, ctx, said);
  return h("div", { class: "party-guests" }, guests.length ? h("ul", { class: "plain" }, guests) : null, h("div", { class: "form-actions" }, invite, tunnel.button, preview), link, tunnel.box, said);
}

// Joining with a code: the party's file opens and plays where the party is.
function joinForm(ctx) {
  const input = h("input", { id: "party-code", autocomplete: "off", spellcheck: "false", maxlength: "9", placeholder: "ABC123" });
  const said = h("p", { class: "note", role: "status" });
  const form = h("form", { class: "search-row", novalidate: true }, h("label", { for: input.id }, "Join with a code"), input, h("button", { type: "submit", class: "quiet" }, "Join"));
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    try {
      took(await timed(clock, () => api("/watch-parties/join", { method: "POST", body: { code: input.value } })));
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
