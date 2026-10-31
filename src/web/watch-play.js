// What Play does for a title, said on the button: "Resume S2:E4 · 23m left", "Play S3:E1",
// "Play · 1 h 52 min". Known at once for what the Watch home already says (progress, the next
// episode); otherwise looked up once (the movie's file, the series' first episode) and kept for
// the page's life.

import { SignedOut, api, runtime } from "/web/lib.js";
import { playHref } from "/web/cards.js";

// "S2:E4", as the apps print it.
export function episodeCode(season, episode) {
  if (season === null || season === undefined || episode === null || episode === undefined) return null;
  return season === 0 ? `Special ${episode}` : `S${season}:E${episode}`;
}

// "23m left", "1h 12m left": what's left of a file partway through.
export function timeLeft(progress) {
  if (!progress?.duration_seconds || progress.watched) return null;
  const minutes = Math.max(1, Math.round((progress.duration_seconds - progress.position_seconds) / 60));
  const hours = Math.floor(minutes / 60);
  return `${hours ? `${hours}h ${minutes % 60}m` : `${minutes}m`} left`;
}

// Partway through: there's a place to resume from.
export function partway(progress) {
  return Boolean(progress && !progress.watched && progress.position_seconds > 0);
}

// A title's own page.
export function titleHref(item) {
  return `#/${item.kind === "movie" ? "movies" : "series"}/${item.id}`;
}

// Where a landscape tile goes: the episode or the file it's about, played; else the title.
export function playsAt(item) {
  const e = item.episode;
  if (item.kind === "series" && e) return playHref(e.file_id, { series: item.id, season: e.season, episode: e.episode });
  if (item.kind === "movie" && item.progress) return playHref(item.progress.file_id, { movie: item.id });
  return titleHref(item);
}

// The ids of every anime series the user may see, as `/series?anime=only` lists them (wanted
// as anime, or in an anime library): what sorts a series into Anime rather than Shows.
export async function animeIds() {
  const ids = new Set();
  for (let offset = 0; ; offset += 500) {
    const page = await api(`/series?anime=only&limit=500&offset=${offset}`);
    for (const item of page.items) ids.add(item.id);
    if (offset + 500 >= page.total) return ids;
  }
}

const known = new Map();

// { href, label, file_id } for playing `item` (a home or list item: kind, id, episode,
// progress), or a promise of it; null when there's nothing to play.
export function playTarget(item) {
  const key = `${item.kind}:${item.id}`;
  const now = fromItem(item);
  if (now) return now;
  if (!known.has(key)) {
    known.set(
      key,
      lookUp(item).catch((err) => {
        known.delete(key);
        if (err instanceof SignedOut) throw err;
        return null;
      }),
    );
  }
  return known.get(key);
}

// What the item itself says, when that's enough.
function fromItem(item) {
  const { episode, progress } = item;
  if (item.kind === "series" && episode?.file_id) {
    const code = episodeCode(episode.season, episode.episode);
    const href = playHref(episode.file_id, { series: item.id, season: episode.season, episode: episode.episode });
    const file_id = episode.file_id;
    if (partway(progress) && progress.file_id === episode.file_id) {
      return { href, label: ["Resume", [code, timeLeft(progress)].filter(Boolean).join(" · ")].join(" "), file_id };
    }
    return { href, label: code ? `Play ${code}` : "Play", file_id };
  }
  if (item.kind === "movie" && progress?.file_id) {
    const { file_id } = progress;
    const href = playHref(file_id, { movie: item.id });
    if (partway(progress)) return { href, label: `Resume · ${timeLeft(progress)}`, file_id };
    if (progress.watched) return { href: playHref(file_id, { movie: item.id }, true), label: "Play again", file_id };
  }
  return null;
}

// The movie's first file, or the series' first episode on disk (Specials last).
async function lookUp(item) {
  if (item.kind === "movie") {
    const movie = await api(`/movies/${item.id}`);
    const file = movie.files[0];
    if (!file) return null;
    const length = runtime(movie.runtime_minutes);
    return { href: playHref(file.id, { movie: item.id }), label: length ? `Play · ${length}` : "Play", file_id: file.id };
  }
  const series = await api(`/series/${item.id}`);
  const seasons = series.seasons.filter((s) => s.available > 0).sort((a, b) => (a.season === 0) - (b.season === 0) || a.season - b.season);
  if (!seasons.length) return null;
  const season = await api(`/series/${item.id}/seasons/${seasons[0].season}`);
  const first = season.episodes.find((e) => e.files.length);
  if (!first) return null;
  const code = episodeCode(seasons[0].season, first.episode);
  return {
    href: playHref(first.files[0].id, { series: item.id, season: seasons[0].season, episode: first.episode }),
    label: `Play ${code}`,
    file_id: first.files[0].id,
  };
}
