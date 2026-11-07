// Styled ASS/SSA subtitles drawn as they were made, signs, placement and karaoke included, by
// JASSUB (libass in WebAssembly, in a worker) on a canvas over the video. Melete serves JASSUB
// and the track and fonts it reads; it asks nothing of anywhere else. Where it can't start, the
// track's WebVTT version is shown instead.

const JASSUB = "/web/vendor/jassub-2.5.21";
// A worker that can't load never says so; past this, the WebVTT version is shown instead.
const START_WITHIN_MS = 15000;

let module = null;

// Whether this browser has what drawing them takes: workers, WebAssembly and a canvas a worker
// can draw on.
export function canDrawAss() {
  return typeof Worker === "function" && typeof WebAssembly === "object" && "transferControlToOffscreen" in HTMLCanvasElement.prototype;
}

// The track the server should draw into the picture when it's styled, for a browser that can't
// draw ASS/SSA itself: `track`'s id when it's an ASS/SSA track and this browser can't, else null.
export function serverDraws(track) {
  return track?.ass && !canDrawAss() ? track.track : null;
}

// Keeps the drawing in step with the chosen track: draws `track` (one the subtitle list marks
// `ass`) over the player's video, or nothing when it's null. When drawing can't start,
// `cantDraw(track)` is called so the player shows the WebVTT version instead.
export function drawStyled(s, track, cantDraw) {
  if ((s.styled?.track ?? null) === track) return;
  s.styled?.stop();
  s.styled = null;
  if (!track) return;
  const drawing = { track, stop: () => {} };
  s.styled = drawing;
  start(s.video, s.streamBase, track).then(
    (renderer) => {
      if (s.styled !== drawing) return renderer.destroy();
      drawing.stop = () => renderer.destroy();
      drawing.scale = sizer(renderer);
      drawing.scale(s.subtitleScale ?? 1);
    },
    (err) => {
      if (s.styled !== drawing) return;
      s.styled = null;
      console.warn("Styled subtitles couldn't be drawn:", err);
      cantDraw(track);
    },
  );
}

// Draws styled subtitles at `scale` times their styles' font sizes (the Subtitles panel's
// size), now and for each track drawn after.
export function scaleStyled(s, scale) {
  s.subtitleScale = scale;
  s.styled?.scale?.(scale);
}

// What sets `renderer`'s styles to `scale` times their font sizes as the track made them, and
// draws again so a paused picture shows it too.
function sizer(renderer) {
  let made = null;
  let now = 1;
  return async (scale) => {
    if (scale === now) return;
    now = scale;
    try {
      made ??= await renderer.renderer.getStyles();
      await Promise.all(made.map((style, i) => renderer.renderer.setStyle({ ...style, FontSize: style.FontSize * scale }, i)));
      await renderer.resize(true);
    } catch (err) {
      console.warn("Styled subtitles couldn't be resized:", err);
    }
  };
}

// Stops drawing, on leaving the player.
export function stopStyled(s) {
  s.styled?.stop();
  s.styled = null;
}

// A JASSUB drawing `track` over `video`, once it's ready, with the fonts the video carries; text
// in a font neither has falls back to the bundled Liberation Sans.
async function start(video, streamBase, track) {
  module ??= import(`${JASSUB}/jassub.mjs`);
  const { default: Renderer } = await module;
  const fonts = await fetch(`${streamBase}/fonts`)
    .then((answer) => (answer.ok ? answer.json() : []))
    .catch(() => []);
  const renderer = new Renderer({
    video,
    subUrl: `${streamBase}/${track.ass_url}`,
    workerUrl: `${JASSUB}/jassub-worker.mjs`,
    wasmUrl: `${JASSUB}/jassub-worker.wasm`,
    modernWasmUrl: `${JASSUB}/jassub-worker-modern.wasm`,
    fonts: fonts.map((font) => font.url),
    // Never the browser's own fonts or an online font service: only what Melete serves.
    queryFonts: false,
  });
  let timer;
  const late = new Promise((_, refuse) => {
    timer = setTimeout(() => refuse(new Error("JASSUB didn't start")), START_WITHIN_MS);
  });
  try {
    await Promise.race([renderer.ready, late]);
  } catch (err) {
    renderer.destroy();
    throw err;
  } finally {
    clearTimeout(timer);
  }
  return renderer;
}
