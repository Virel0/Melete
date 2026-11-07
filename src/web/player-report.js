// A playback report: what the player saw while it played (waiting and stalls, errors, the way it
// played and each conversion asked for, conversion errors, seeks and subtitle changes), the last
// 200 of them, sent to the admins' Diagnostics when the viewer asks, from the player's Settings
// panel or Your account. Nothing is sent otherwise. A report carries no addresses: a stream's
// address can carry a session, so only what kind of error it was is kept.
//
// playbackLog(video, fileId) → { note(event, detail) } starts a play's log; reportButton(log,
// ctx) is the player's button; reportGroup(ctx) is Your account's part, for the last play in
// this tab.

import { group } from "/web/forms.js";
import { SignedOut, api, h } from "/web/lib.js";

// The most kept, oldest dropped first.
const MOST = 200;
// Room the report leaves under the server's 64 KiB.
const MOST_BYTES = 60_000;

// The latest play's log in this tab, for Your account.
let latest = null;

export function playbackLog(video, fileId) {
  const log = { fileId, started: Date.now(), events: [], picture: null };
  latest = log;
  let position = 0;
  let waitingSince = null;
  const note = (event, detail = {}) => {
    const at = Number.isFinite(video.currentTime) ? Math.round(video.currentTime * 10) / 10 : null;
    log.events.push({ ms: Date.now() - log.started, at, event, ...detail });
    if (log.events.length > MOST) log.events.shift();
  };
  video.addEventListener("timeupdate", () => {
    if (!video.seeking) position = video.currentTime;
  });
  video.addEventListener("seeking", () => note("seek", { from: Math.round(position * 10) / 10 }));
  video.addEventListener("waiting", () => {
    waitingSince ??= Date.now();
    note("waiting");
  });
  video.addEventListener("stalled", () => note("stalled"));
  video.addEventListener("playing", () => {
    if (waitingSince !== null) note("playing", { waited_ms: Date.now() - waitingSince });
    waitingSince = null;
  });
  video.addEventListener("error", () => note("error", { code: video.error?.code ?? null, message: video.error?.message || null }));
  video.addEventListener("ended", () => note("ended"));
  video.addEventListener("loadedmetadata", () => {
    log.picture = { width: video.videoWidth || null, height: video.videoHeight || null, duration: Number.isFinite(video.duration) ? video.duration : null };
  });
  // Subtitles shown as text tracks; drawn-in ones show in the conversion asked for.
  let subtitles = null;
  video.textTracks.addEventListener("change", () => {
    const shown = [...video.textTracks].find((track) => track.mode === "showing");
    const now = shown ? shown.label || shown.language || "unnamed" : null;
    if (now !== subtitles) note("subtitles", { showing: (subtitles = now) });
  });
  log.note = note;
  return log;
}

// What goes to the server: the events, newest kept when it's too large.
function report(log, from) {
  const count = (...events) => log.events.filter((e) => events.includes(e.event)).length;
  const errors = count("error", "conversion_error");
  const stalls = count("stalled");
  const waits = count("waiting");
  const body = {
    app: "Melete web",
    platform: (navigator.userAgentData?.platform || navigator.platform || "").slice(0, 64) || null,
    level: errors ? "error" : stalls ? "warning" : "info",
    summary: `Playback report for file ${log.fileId}: ${errors} errors, ${stalls} stalls, ${waits} waits.`,
    details: {
      sent_from: from,
      file_id: log.fileId,
      browser: navigator.userAgent,
      screen: `${screen.width}×${screen.height}`,
      video: log.picture,
      events: [...log.events],
    },
  };
  while (JSON.stringify(body).length > MOST_BYTES && body.details.events.length) body.details.events.shift();
  return body;
}

// A button that sends `log()`'s report, with what happened said beside it.
function sendButton(log, from, onSignedOut) {
  const said = h("p", { class: "form-result", role: "status" });
  const button = h("button", { type: "button", class: "quiet" }, "Send a playback report");
  button.addEventListener("click", async () => {
    const chosen = log();
    if (!chosen) return;
    button.disabled = true;
    said.textContent = "Sending…";
    said.classList.remove("error");
    try {
      await api("/me/diagnostics", { method: "POST", body: report(chosen, from) });
      said.textContent = "Sent. Admins can read it in Settings → Diagnostics.";
    } catch (err) {
      if (err instanceof SignedOut) return onSignedOut();
      said.textContent = `Not sent: ${err.message}`;
      said.classList.add("error");
    } finally {
      button.disabled = false;
    }
  });
  return h("div", { class: "form-actions playback-report" }, button, said);
}

// The player's button, for the Settings panel.
export function reportButton(log, ctx) {
  return sendButton(() => log, "player", () => ctx.signedOut());
}

// Your account's part: the report of the last thing played in this tab, sent on request.
export function reportGroup(ctx) {
  if (!latest) {
    return group("Playback report", h("p", { class: "note" }, "Play something in this tab first. A report is what the player saw while it played (waiting, errors, how it played, seeks); it goes to the admins' Diagnostics only when you send it."));
  }
  return group(
    "Playback report",
    h("p", { class: "note" }, "What the player saw the last time it played something in this tab: waiting, errors, how it played and seeks. It goes to the admins' Diagnostics only when you send it."),
    sendButton(() => latest, "account", () => ctx.signedOut()),
  );
}
