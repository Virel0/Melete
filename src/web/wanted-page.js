// Wanted, for admins: the wanted movies, series and music, searching indexers (and Soulseek,
// for music) and grabbing a result, downloads, RSS syncs, and files upgrades replaced. What
// Sonarr's, Radarr's and Lidarr's Wanted and Activity pages did. This file only lists the
// sections, so the ones that show a search's results can import those from wanted.js without
// wanted.js importing them back.

import { sectioned } from "/web/forms.js";
import { activity } from "/web/progress.js";
import { wantedList } from "/web/wanted.js";
import { downloads } from "/web/wanted-downloads.js";
import { wantedArtists, wantedSongs } from "/web/wanted-music.js";
import { rss } from "/web/wanted-rss.js";
import { trash } from "/web/wanted-set-aside.js";

const SECTIONS = [
  { key: "activity", name: "Activity", admin: true, page: activity },
  { key: "movies", name: "Movies", admin: true, page: (ctx) => wantedList(ctx, "movies") },
  { key: "series", name: "Series", admin: true, page: (ctx) => wantedList(ctx, "series") },
  { key: "music", name: "Music", admin: true, page: wantedArtists },
  { key: "songs", name: "Songs", admin: true, page: wantedSongs },
  { key: "downloads", name: "Downloads", admin: true, page: downloads },
  { key: "rss", name: "RSS syncs", admin: true, page: rss },
  { key: "trash", name: "Trash", admin: true, page: trash },
];

export function wantedPage(section, ctx) {
  return sectioned({ title: "Wanted", base: "#/wanted", sections: SECTIONS }, section, ctx);
}
