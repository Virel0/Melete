// A title page's header plays a muted preview behind its words after a few seconds in view, as
// the Watch home's hero does, by the same rules (watch-hero-trailer.js: the viewer's hero
// trailers setting, a clip only on the home network, nothing when the device asks for less
// motion, Unmute and Stop beside the buttons). The preview is the title's trailer when it's
// sharp enough to stand in for the title: at least 1080 lines tall, or as tall as the title's
// own tallest file where that's lower (one not measured yet counts). Otherwise it's a clip of
// the title itself. It stops once the header scrolls out of view, and waits again when it's back.
//
// headerPreview(header, item, { extras, files }) → a function ending it for good, for leaving. `header` is the
// page's `.hero`; `item` is { kind, id, runtime_minutes, progress, episode } as the hero's picks
// have them, so a clip starts where the viewer left off; `files` are the ids of the title's own
// files to measure the trailer against (a movie's files, or the episode Play goes to).

import { api } from "/web/lib.js";
import { heroTrailer } from "/web/watch-hero-trailer.js";

const stillness = matchMedia("(prefers-reduced-motion: reduce)");

// The tallest picture in a file, or null when it isn't measured yet (or can't be read).
async function height(fileId) {
  try {
    const info = await api(`/files/${fileId}`);
    const heights = (info.media?.video ?? []).map((v) => v.height).filter(Boolean);
    return heights.length ? Math.max(...heights) : null;
  } catch {
    return null;
  }
}

// The id of the title's trailer when it's sharp enough to stand in for the title, else null.
async function sharpTrailer(extras, files) {
  const trailer = extras.find((e) => e.kind === "trailer");
  if (!trailer) return null;
  const tall = await height(trailer.id);
  if (tall === null) return trailer.id;
  const own = (await Promise.all(files.map(height))).filter((n) => n !== null);
  const needed = Math.min(1080, own.length ? Math.max(...own) : 1080);
  return tall >= needed ? trailer.id : null;
}

export function headerPreview(header, item, { extras, files }) {
  const trailer = heroTrailer(header.querySelector(".hero-art"));
  header.querySelector(".hero-body > .actions").after(trailer.el);
  let full = null;
  let inView = false;
  const start = async () => {
    // Nothing to work out for a device that asks for less motion: it plays none.
    if (stillness.matches) return;
    full ??= sharpTrailer(extras, files).then((id) => ({ ...item, trailer: id ? { source: "folder", file_id: id } : null }));
    const ready = await full;
    if (inView && !trailer.playing()) trailer.show(ready);
  };
  const seen = new IntersectionObserver(
    ([entry]) => {
      inView = entry.isIntersecting;
      if (inView) start();
      else trailer.stop();
    },
    { threshold: 0.5 },
  );
  seen.observe(header);
  return () => {
    seen.disconnect();
    trailer.close();
  };
}
