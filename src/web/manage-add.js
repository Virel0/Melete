// Add new, in Manage's Movies, Series, Anime and Music: a search that finds as you type, every
// kind the service has at once (manage-find.js, through /requests/search, and Soulseek's
// /requests/soulseek for music), and each result added with where it goes
// (manage-add-form.js). Then the import from Radarr, Sonarr or Lidarr, which brings their whole
// list. And Needs a match: the service's titles TMDB or MusicBrainz couldn't settle, matched by
// hand.

import { api } from "/web/lib.js";
import { KINDS, profilesFor } from "/web/manage-parts.js";
import { findPart } from "/web/manage-find.js";
import { arrs } from "/web/sources-arrs.js";
import { lidarrArtists, musicMatching } from "/web/admin-music.js";
import { matching } from "/web/settings.js";

const LIBRARY_KIND = { movies: "movies", series: "shows", anime: "shows", music: "music" };

export async function addPage(ctx) {
  const key = ctx.service.key;
  const [libraries, profiles] = await Promise.all([api("/libraries"), profilesFor(KINDS[key]).catch(() => [])]);
  if (!ctx.current()) return [];
  // Anime offers its anime libraries first; Series leaves them out.
  const mine = libraries
    .filter((l) => l.kind === LIBRARY_KIND[key] && !(key === "series" && l.category === "anime"))
    .sort((a, b) => (key === "anime" ? (b.category === "anime") - (a.category === "anime") : 0));
  const page = { ctx, key, libraries: mine, profiles };
  const imports = key === "music" ? [lidarrArtists()] : await arrs({ ...ctx, kind: key === "movies" ? "movies" : "series" });
  return [findPart(page), ...imports];
}

// Needs a match: the titles of this service TMDB couldn't settle (a series in an anime library
// is Anime's), or the albums MusicBrainz couldn't, for Music.
export async function needsMatch(ctx) {
  const key = ctx.service.key;
  if (key === "music") return musicMatching(ctx);
  const anime = new Set((await api("/libraries")).filter((l) => l.category === "anime").map((l) => l.name));
  const only = {
    movies: (item) => item.kind === "movie",
    series: (item) => item.kind === "series" && !anime.has(item.library),
    anime: (item) => item.kind === "series" && anime.has(item.library),
  }[key];
  return matching({ ...ctx, matchOnly: only });
}
