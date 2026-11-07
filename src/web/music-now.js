// Now Playing's look in the panel, from the playing track's album (`GET /music/albums/{id}`,
// asked once an album): its cover when the track came without one (a song from a list), its
// colours from the cover (the full-screen panel takes its ground and text from them: text1 reads
// at 4.5:1 or more on the background, text2 at 3:1, worked out on the server), and its motion
// cover looping muted over the still one (music-motion.js), paused while the panel is closed.
//
// nowLook(coverBox, img, panel) → { show(track), pause() }.

import { api } from "/web/lib.js";
import { motionCover } from "/web/music-motion.js";

// Albums asked for, by id: the answer once it's in.
const albums = new Map();
const KEPT = 50;

async function album(id) {
  if (albums.has(id)) return albums.get(id);
  const answer = await api(`/music/albums/${id}`).catch(() => null);
  if (answer) {
    if (albums.size >= KEPT) albums.delete(albums.keys().next().value);
    albums.set(id, answer);
  }
  return answer;
}

export function nowLook(coverBox, img, panel) {
  let shown = null;
  let asked = 0;
  let video = null;

  const picture = (src) => {
    img.hidden = !src;
    if (src && img.getAttribute("src") !== src) img.src = src;
  };
  const paint = (colours) => {
    panel.classList.toggle("coloured", Boolean(colours));
    const set = { "--np-bg": colours?.background, "--np-ink": colours?.text1, "--np-soft": colours?.text2 };
    for (const [name, value] of Object.entries(set)) {
      if (value) panel.style.setProperty(name, value);
      else panel.style.removeProperty(name);
    }
  };

  const show = async (track) => {
    const id = track?.album_id ?? null;
    picture(track?.cover ?? albums.get(id)?.cover ?? null);
    if (id === shown) return video?.isConnected && video.play().catch(() => {});
    shown = id;
    const mine = ++asked;
    video?.remove();
    video = null;
    paint(null);
    if (!id) return;
    const answer = await album(id);
    if (mine !== asked || !answer) return;
    if (!track.cover) picture(answer.cover);
    paint(answer.colours);
    video = motionCover(answer.motion_cover);
    if (video) coverBox.append(video);
  };

  return { show, pause: () => video?.pause() };
}
