// Playing a file in the browser: the file as it is when the browser can play it, otherwise an
// HLS conversion from the server (only the container and audio changed when the video itself
// plays). Text subtitles come as WebVTT tracks; picture subtitles are drawn into a conversion.
// Progress goes back to the server every 10 seconds and on leaving.

import { SignedOut, announce, api, clock, episodeLabel, h, loading, show } from "/web/lib.js";
import { SOUNDS, audioLabel, choose, describe, mse, probe, subtitleLabel } from "/web/player-choice.js";
import { playerKeys, shortcutsHelp, stageFullscreen } from "/web/player-keys.js";
import { nextSubtitle, subtitleChips } from "/web/player-chips.js";
import { episodeHref, episodeLinks, skipButtons, upNextPanel } from "/web/player-next.js";
import { offlinePanel } from "/web/player-offline.js";
import { previewBar } from "/web/player-previews.js";
import { subtitleSearch } from "/web/player-subtitle-search.js";

// Vendored, unchanged, from the hls.js 1.7.3 npm release. Loaded only when a
// conversion plays in a browser without HLS of its own.
const HLS_MODULE = "/web/hls-1.7.3.mjs";
const REPORT_EVERY_MS = 10_000;

// ---------- The page ----------

// `params`: movie=ID, or series=ID&season=S&episode=E, or series=ID alone for one of its
// extras; start=0 to play from the beginning.
export async function playerPage(fileId, params, ctx) {
  const movieId = params.get("movie");
  const seriesId = params.get("series");
  const seasonNumber = params.has("season") ? Number(params.get("season")) : null;
  const episodeNumber = Number(params.get("episode"));
  show("Play", loading("the video"));

  const { info, play, subtitles, title, season, extras } = await fetchPlaying(fileId, movieId, seriesId, seasonNumber);
  // The recap, intro, credits and preview found in the episode, if any; each gets a Skip.
  const segments = play.segments;
  if (!ctx.current()) return;

  const episode = season?.episodes.find((e) => e.episode === episodeNumber);
  // A trailer or other extra keeps no progress, and is named after its title.
  const extra = extras?.find((e) => e.id === Number(fileId));
  const heading = extra ? `${title.title}: ${extra.title}` : (title?.title ?? info.path.split("/").pop());
  const seasonQuery = seasonNumber === null ? "" : `?season=${seasonNumber}`;
  const backHref = movieId ? `#/movies/${movieId}` : seriesId ? `#/series/${seriesId}${seasonQuery}` : "#/";
  // Starting over is said with each report, or the server keeps a saved position over one
  // near the start.
  const restart = params.get("start") === "0";
  const start = restart ? 0 : play.resume_seconds;

  const textTracks = subtitles.filter((t) => t.text);
  // What the player's parts share: the video, the choices made, and the source playing.
  const s = {
    ctx,
    fileId,
    info,
    start,
    textTracks,
    streamBase: play.stream_url,
    video: videoWithTracks(play.stream_url, textTracks),
    status: h("p", { class: "status", role: "status" }),
    seriesId,
    subtitles,
    choice: { ...startingChoice(play.tracks, subtitles), sound: null },
    mode: null,
    hls: null,
    // Bumped by each load, so a slow answer for choices already changed again is dropped.
    loads: 0,
    keepsProgress: !extra,
  };
  const video = s.video;
  convertWhenRefused(s);
  const subtitlesBox = subtitlePicker(s);
  const controls = [audioPicker(s), soundPicker(s), subtitlesBox, chapterPicker(s)].filter(Boolean);
  const pickSubtitles = subtitlesBox?.querySelector("select") ?? null;
  const reports = startReports(s, restart);
  const where = { seriesId, seasonNumber, episodeNumber, season, episode, title };
  const episodes = episodeLinks(where, ctx);
  const upNext = upNextPanel(video, where, episodes.after, { ctx, report: reports.report, backHref, heading, credits: segments?.credits });
  const { stage, fullscreen, onKey, onFullscreen } = framed(s, { segments, episodes, upNext, pickSubtitles });
  document.addEventListener("keydown", onKey);
  document.addEventListener("fullscreenchange", onFullscreen);

  ctx.onLeave(() => {
    reports.report(true);
    clearInterval(reports.timer);
    upNext.cancel();
    removeEventListener("pagehide", reports.onHide);
    document.removeEventListener("keydown", onKey);
    document.removeEventListener("fullscreenchange", onFullscreen);
    detach(s);
    video.load();
  });

  show(
    episode ? `${heading}: ${episodeLabel(seasonNumber, episodeNumber)}` : heading,
    h("p", { class: "back" }, h("a", { href: backHref }, `Back to ${title?.title ?? heading}`)),
    h(
      "div",
      { class: "player-head" },
      h("h1", { class: "player-title" }, heading),
      episode ? h("p", { class: "player-episode" }, episodeLabel(seasonNumber, episodeNumber, episode.name)) : null,
    ),
    stage,
    previewBar(video, fileId, play.stream_url, { trickplay: info.trickplay, chapters: info.chapters ?? [], credits: segments?.credits }),
    subtitleChips(pickSubtitles, video),
    upNext.panel,
    resumedNote(start, fileId, params),
    h(
      "div",
      { class: "player-panel" },
      controls.length ? h("div", { class: "toolbar player-options" }, controls) : null,
      h("div", { class: "player-end" }, episodes.nav, h("button", { type: "button", class: "quiet", onclick: () => fullscreen()?.catch?.(() => {}) }, "Full screen")),
    ),
    info.subtitle_search ? subtitleSearch(fileId, (track) => addTrack(s, subtitles, track)) : null,
    info.offline ? offlinePanel(fileId, ctx) : null,
    s.status,
    shortcutsHelp(),
  );
  await load(s, start);
}

// The stage: the picture and what floats over it (Skip, the next episode), which go full screen
// together; and the keys, with C for the next subtitles and N for the next episode.
function framed(s, { segments, episodes, upNext, pickSubtitles }) {
  const stage = h("div", { class: "stage" }, s.video, h("div", { class: "stage-over" }, skipButtons(s.video, segments), upNext.card));
  const { fullscreen, onFullscreen } = stageFullscreen(s.video, stage);
  const onKey = playerKeys(s.video, {
    subtitles: () => nextSubtitle(pickSubtitles),
    next: () => episodes.after.then((n) => (n ? (location.hash = episodeHref(s.seriesId, n)) : announce("There's no next episode."))),
    fullscreen,
  });
  return { stage, fullscreen, onKey, onFullscreen };
}

// The file, how to play it, its subtitles, and the movie or series and season it belongs to.
async function fetchPlaying(fileId, movieId, seriesId, seasonNumber) {
  const subtitlesOrNone = api(`/files/${fileId}/subtitles`).catch((err) => {
    if (err instanceof SignedOut) throw err;
    return [];
  });
  const owner = movieId ? `/movies/${movieId}` : seriesId ? `/series/${seriesId}` : null;
  const [info, play, subtitles, title, season, extras] = await Promise.all([
    api(`/files/${fileId}`),
    api(`/files/${fileId}/play`, { method: "POST" }),
    subtitlesOrNone,
    movieId ? api(`/movies/${movieId}`) : seriesId ? api(`/series/${seriesId}`) : null,
    seriesId && seasonNumber !== null ? api(`/series/${seriesId}/seasons/${seasonNumber}`) : null,
    // To tell an extra by its id; an episode's link (with its season) is never one.
    owner && seasonNumber === null ? api(`${owner}/extras`) : null,
  ]);
  return { info, play, subtitles, title, season, extras };
}

// The video element, with the text subtitles as WebVTT tracks.
function videoWithTracks(streamBase, textTracks) {
  // Full screen is the stage's (the page's button, F and a double-click), so Skip and the next
  // episode come too.
  const video = h("video", { controls: true, playsinline: true, preload: "metadata", controlslist: "nofullscreen" });
  for (const t of textTracks) {
    video.append(
      h("track", {
        kind: "subtitles",
        src: `${streamBase}/subtitles/${t.track}.vtt`,
        label: subtitleLabel(t),
        srclang: t.language && /^[a-z]{2,3}$/.test(t.language) ? t.language : null,
      }),
    );
  }
  return video;
}

// ---------- Sources ----------

// Stops a conversion's hls.js, if any, and takes the video's source away.
function detach(s) {
  s.hls?.destroy();
  s.hls = null;
  s.video.removeAttribute("src");
}

// Plays a conversion's playlist: through hls.js where the browser has Media Source, otherwise
// the browser's own HLS.
async function attachHls(s, url, at, mine) {
  if (mse) {
    const { default: Hls } = await import(HLS_MODULE);
    if (!s.ctx.current() || mine !== s.loads) return;
    if (Hls.isSupported()) {
      s.hls = new Hls({ enableWorker: false, startPosition: at });
      s.hls.on(Hls.Events.ERROR, (_, data) => {
        if (data.fatal) failed(s, "The converted video stopped loading. Try again, or pick other settings.");
      });
      s.hls.loadSource(url);
      s.hls.attachMedia(s.video);
      return;
    }
  }
  if (probe.canPlayType("application/vnd.apple.mpegurl")) {
    s.video.src = url;
    seekWhenReady(s.video, at);
    return;
  }
  throw new Error("This browser can't play converted video. Try a current Firefox, Chrome or Safari.");
}

// Moves to `at` seconds once the video knows its length.
function seekWhenReady(video, at) {
  if (at > 0) video.addEventListener("loadedmetadata", () => (video.currentTime = at), { once: true });
}

// Puts the source the current choices call for in place, keeping the position.
async function load(s, at) {
  const next = choose(s.info, s.choice);
  if (s.mode && JSON.stringify(next) === JSON.stringify(s.mode)) return;
  s.mode = next;
  const mine = ++s.loads;
  s.status.textContent = s.mode.direct ? "Starting…" : "Asking the server to convert the video…";
  detach(s);
  if (s.mode.direct) {
    s.video.src = s.streamBase;
    seekWhenReady(s.video, at);
  } else {
    const { direct: _, ...request } = s.mode;
    const answer = await api(`/files/${s.fileId}/transcode`, { method: "POST", body: request });
    if (!s.ctx.current() || mine !== s.loads) return;
    await attachHls(s, answer.playlist_url, at, mine);
    if (mine !== s.loads) return;
  }
  s.status.textContent = describe(s.mode);
  showTracks(s);
  // The click that opened the player lets it start with sound; if the browser still says no,
  // its own play button is there.
  s.video.play().catch(() => {});
}

// Says what went wrong under the video, with a button to try again.
function failed(s, message) {
  s.status.textContent = "";
  s.status.append(h("span", { class: "error" }, message), " ", h("button", { type: "button", class: "quiet", onclick: () => retry(s) }, "Try again"));
}

// Loads the source again from where the video was.
function retry(s) {
  const at = s.video.currentTime || s.start;
  s.mode = null;
  load(s, at).catch((err) => onError(s, err));
}

// A load that failed: back to sign-in when signed out, otherwise said under the video.
function onError(s, err) {
  if (err instanceof SignedOut) return s.ctx.signedOut();
  failed(s, err.message);
}

// When the browser can't play the file as it is, after all, the player asks for a conversion.
function convertWhenRefused(s) {
  s.video.addEventListener("error", () => {
    if (!s.mode || !s.ctx.current()) return;
    // The browser turned the file down after all: convert it instead, once.
    if (s.mode.direct && !s.choice.fallback) {
      s.choice.fallback = true;
      load(s, s.video.currentTime || s.start).catch((err) => onError(s, err));
      return;
    }
    if (!s.hls) failed(s, "The video can't be played in this browser.");
  });
}

// ---------- Audio and subtitles ----------

// The tracks to start with: those the server picked for this user (their languages, or what
// they chose earlier in the series), else the default audio with forced subtitles (signs,
// foreign dialogue) on.
function startingChoice(picked, subtitles) {
  const track = picked ? subtitles.find((t) => t.track === picked.subtitle) : subtitles.find((t) => t.forced && t.text);
  return {
    audio: picked?.audio ?? null,
    subtitle: track?.track ?? null,
    burn: track && !track.text ? track.track : null,
    fallback: false,
  };
}

// Tells the server what was chosen by hand in an episode, so the series' next ones start so.
function rememberChoice(s) {
  if (!s.seriesId) return;
  const audio = (s.info.media?.audio ?? []).find((a) => a.index === s.choice.audio);
  const subtitle = s.subtitles.find((t) => t.track === s.choice.subtitle);
  const body = {
    audio_language: audio?.language ?? null,
    subtitle: subtitle ? { language: subtitle.language ?? null, forced: subtitle.forced, hearing_impaired: subtitle.hearing_impaired } : null,
  };
  // Playing goes on whether or not it's kept.
  api(`/files/${s.fileId}/track-choice`, { method: "PUT", body }).catch(() => {});
}

// The audio track picker, when the file has more than one.
function audioPicker(s) {
  const audioStreams = s.info.media?.audio ?? [];
  if (audioStreams.length <= 1) return null;
  const defaultAudio = audioStreams.find((a) => a.default) ?? audioStreams[0];
  const starting = audioStreams.find((a) => a.index === s.choice.audio) ?? defaultAudio;
  const select = h(
    "select",
    { id: "audio-track" },
    audioStreams.map((a) => h("option", { value: a.index, selected: a === starting }, audioLabel(a))),
  );
  select.addEventListener("change", () => {
    s.choice.audio = Number(select.value);
    rememberChoice(s);
    load(s, s.video.currentTime).catch((err) => onError(s, err));
    announce(`Audio: ${select.selectedOptions[0].textContent}`);
  });
  return h("div", {}, h("label", { for: "audio-track" }, "Audio"), select);
}

// The file's own chapters to jump to, when it has more than one.
function chapterPicker(s) {
  const chapters = s.info.chapters ?? [];
  if (chapters.length < 2) return null;
  const select = h(
    "select",
    { id: "chapter" },
    h("option", { value: "" }, "Jump to…"),
    chapters.map((c, i) => h("option", { value: i }, `${clock(c.start_seconds)} ${c.title ?? `Chapter ${i + 1}`}`)),
  );
  select.addEventListener("change", () => {
    if (select.value === "") return;
    const chapter = chapters[Number(select.value)];
    s.video.currentTime = chapter.start_seconds;
    announce(`Chapter: ${select.selectedOptions[0].textContent}`);
    select.value = "";
  });
  return h("div", {}, h("label", { for: "chapter" }, "Chapters"), select);
}

// How the sound is made, when the file has any: as mixed, or converted by the server for clearer
// dialogue or an even volume.
function soundPicker(s) {
  if (!s.info.media?.audio.length) return null;
  const select = h(
    "select",
    { id: "sound-mode" },
    SOUNDS.map((sound) => h("option", { value: sound.mode }, sound.label)),
  );
  select.addEventListener("change", () => {
    s.choice.sound = select.value || null;
    load(s, s.video.currentTime).catch((err) => onError(s, err));
    announce(`Sound: ${select.selectedOptions[0].textContent}`);
  });
  return h("div", {}, h("label", { for: "sound-mode" }, "Sound"), select);
}

// The subtitle picker, kept in step with the video's own captions menu; shown when there are
// subtitles.
function subtitlePicker(s) {
  const subtitles = s.subtitles;
  const subtitleSelect = h(
    "select",
    { id: "subtitle-track" },
    h("option", { value: "" }, "Off"),
    subtitles.map((t) => h("option", { value: t.track }, subtitleLabel(t))),
  );
  subtitleSelect.value = s.choice.subtitle ?? "";
  subtitleSelect.addEventListener("change", () => {
    const picked = subtitles.find((t) => t.track === subtitleSelect.value) ?? null;
    s.choice.subtitle = picked?.track ?? null;
    s.choice.burn = picked && !picked.text ? picked.track : null;
    rememberChoice(s);
    showTracks(s);
    load(s, s.video.currentTime).catch((err) => onError(s, err));
  });
  // The player's own captions menu changes tracks too; keep the list in step. Showing the
  // chosen track fires this as well, which changes nothing and so isn't remembered.
  s.video.textTracks.addEventListener("change", () => {
    if (s.choice.burn) return;
    const i = [...s.video.textTracks].findIndex((track) => track.mode === "showing");
    const shown = i >= 0 ? s.textTracks[i].track : null;
    if (shown === s.choice.subtitle) return;
    s.choice.subtitle = shown;
    subtitleSelect.value = s.choice.subtitle ?? "";
    rememberChoice(s);
  });
  const shown = subtitles.length || s.info.subtitle_search;
  return shown ? h("div", {}, h("label", { for: "subtitle-track" }, "Subtitles"), subtitleSelect) : null;
}

// A text track fetched while playing: added to the video and the subtitle list, and shown.
function addTrack(s, subtitles, track) {
  if (!subtitles.some((t) => t.track === track.track)) {
    subtitles.push(track);
    s.textTracks.push(track);
    s.video.append(h("track", { kind: "subtitles", src: `${s.streamBase}/subtitles/${track.track}.vtt`, label: subtitleLabel(track) }));
    document.getElementById("subtitle-track")?.append(h("option", { value: track.track }, subtitleLabel(track)));
  }
  s.choice.subtitle = track.track;
  s.choice.burn = null;
  const select = document.getElementById("subtitle-track");
  if (select) select.value = track.track;
  showTracks(s);
}

// Shows the chosen text track, and none when a picture subtitle is drawn into the video.
function showTracks(s) {
  const shownIndex = s.choice.burn ? -1 : s.textTracks.findIndex((t) => t.track === s.choice.subtitle);
  [...s.video.textTracks].forEach((track, i) => (track.mode = i === shownIndex ? "showing" : "disabled"));
}

// ---------- Progress ----------

// Reports the position every 10 seconds while playing, on pausing, and as the tab goes. Returns
// `report`, and the timer and pagehide listener to stop on leaving.
function startReports(s, restart) {
  let lastReported = -1;
  const report = (keepalive = false) => {
    const position = s.video.currentTime;
    if (!s.keepsProgress || !s.mode || !Number.isFinite(position) || position === lastReported) return;
    lastReported = position;
    const duration = Number.isFinite(s.video.duration) && s.video.duration > 0 ? s.video.duration : null;
    api(`/files/${s.fileId}/progress`, {
      method: "PUT",
      body: {
        position_seconds: Math.min(position, 604800),
        duration_seconds: duration && Math.min(duration, 604800),
        restart,
      },
      keepalive,
    }).catch((err) => {
      if (err instanceof SignedOut) s.ctx.signedOut();
    });
  };
  const timer = setInterval(() => {
    if (!s.video.paused) report();
  }, REPORT_EVERY_MS);
  s.video.addEventListener("pause", () => report());
  const onHide = () => report(true);
  addEventListener("pagehide", onHide);
  return { report, timer, onHide };
}

// ---------- Picking up where it was left ----------

// "Picking up at 12:34", with a button to start from the beginning instead.
function resumedNote(start, fileId, params) {
  return start > 0
    ? h(
        "p",
        { class: "resumed" },
        `Picking up at ${clock(start)}. `,
        h(
          "button",
          {
            type: "button",
            class: "link",
            // Starting over is its own address, so the server is told with every report.
            onclick: () => {
              const again = new URLSearchParams(params);
              again.set("start", "0");
              location.replace(`#/play/${fileId}?${again}`);
            },
          },
          "Start from the beginning",
        ),
      )
    : null;
}
