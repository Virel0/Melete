// An album's page in Manage → Music: its cover, artist, monitoring, state, type and release;
// Search and Search and choose for a wanted one; its tracks by disc with their format once
// it's in the library; and what happened to it. Reached as `#/admin/music/album/<wanted album
// id>?artist=<wanted artist id>`, or "f" and a library album's id for one that isn't wanted.

import { SignedOut, api, clock, h, plural } from "/web/lib.js";
import { emptyState, failed, stateLabel } from "/web/page-parts.js";
import { monitorSwitch } from "/web/manage-parts.js";
import { searchAndChoose, searchAndGrab } from "/web/manage-search.js";
import { eventRow } from "/web/manage-history.js";
import { albumState } from "/web/manage-artist.js";
import { art } from "/web/cards.js";

// A wanted album with its artist's managed row and its downloads under way.
async function loadWanted(albumId, artistId) {
  const [albums, rows, loads] = await Promise.all([
    api(`/wanted/artists/${artistId}/albums`).catch((err) => (err.status === 404 ? [] : Promise.reject(err))),
    api(`/manage/artists?wanted_id=${artistId}&limit=1`),
    api("/downloads"),
  ]);
  const wanted = albums.find((a) => a.id === Number(albumId)) ?? null;
  const downloads = loads.filter((d) => d.wanted_album_id === wanted?.id && d.state !== "imported");
  return { wanted, artist: rows.items[0] ?? null, downloads };
}

export async function albumPage(ctx) {
  const folder = /^f\d+$/.test(ctx.id ?? "") ? ctx.id.slice(1) : null;
  const artistId = ctx.params.get("artist");
  const asked = !folder && /^\d+$/.test(ctx.id ?? "") && /^\d+$/.test(artistId ?? "");
  const { wanted, artist, downloads } = asked ? await loadWanted(ctx.id, artistId) : { wanted: null, artist: null, downloads: [] };
  const albumId = folder ?? wanted?.album_id;
  const here = albumId ? await api(`/music/albums/${albumId}`).catch((err) => (err.status === 404 ? null : Promise.reject(err))) : null;
  if (!ctx.current()) return [];
  if (!wanted && !here) return [h("h1", {}, "Not found"), emptyState("Music has no such album. It may have been taken off the list.", { href: "#/admin/music/library", label: "Go to Music" })];
  const title = wanted?.title ?? here.title;
  ctx.setTitle(title);
  const v = { ctx, wanted, artist, here, downloads, t: artist ?? { monitored: true } };
  return [header(v, title), tracksPart(v), wanted && artist ? albumHistory(v, title) : null];
}

function header(v, title) {
  const { ctx, wanted, artist, here } = v;
  const said = h("p", { class: "form-result", role: "status" });
  const results = h("div", { class: "title-panel" });
  const box = art(here?.cover ?? null);
  box.classList.add("square");
  const by = artist ? h("a", { href: `#/admin/music/artist/${artist.wanted_id}` }, artist.title) : here ? h("a", { href: `#/admin/music/artist/f${here.artist_id}` }, here.artist) : null;
  const [mark, words] = wanted ? albumState(v, wanted) : ["have", "In the library, not wanted"];
  const facts = [
    wanted ? ["Type", [wanted.album_type ?? "Unknown", ...wanted.secondary_types].join(", ")] : null,
    wanted?.release_date ? ["Released", wanted.release_date] : here?.year ? ["Released", String(here.year)] : null,
    here ? ["Tracks", plural(here.track_list.length, "track", "tracks")] : null,
    here?.library ? ["Library", here.library] : null,
  ].filter(Boolean);
  const actions = h("div", { class: "row-actions title-actions" });
  if (wanted) {
    const ask = { path: `/wanted/albums/${wanted.id}/search`, body: undefined, label: `${title}${artist ? ` by ${artist.title}` : ""}` };
    const search = h("button", { type: "button" }, "Search");
    search.addEventListener("click", async () => {
      search.disabled = true;
      await searchAndGrab(ctx, ask, said);
      search.disabled = false;
    });
    actions.append(search, h("button", { type: "button", class: "quiet", onclick: () => searchAndChoose(ctx, ask, results) }, "Search and choose"));
  }
  if (here) actions.append(h("a", { class: "button quiet", href: `#/music/albums/${here.id}` }, "Open in Listen"));
  return [
    h(
      "div",
      { class: "manage-head" },
      box,
      h(
        "div",
        { class: "manage-head-facts" },
        h(
          "div",
          { class: "title-line" },
          h("h1", {}, title),
          wanted ? monitorSwitch({ ctx, label: `Monitor ${title}`, on: wanted.monitored, shown: true, said, change: (on) => api(`/wanted/albums/${wanted.id}/monitored`, { method: "PUT", body: { monitored: on } }) }) : null,
        ),
        by ? h("p", { class: "by-line" }, "by ", by) : null,
        h("p", { class: "title-state" }, stateLabel(mark, words)),
        h("dl", { class: "kv" }, facts.map(([k, val]) => [h("dt", {}, k), h("dd", {}, val)])),
        actions,
        said,
      ),
    ),
    results,
  ];
}

// Its tracks, disc by disc, each with its length and format.
function tracksPart(v) {
  const tracks = v.here?.track_list ?? [];
  if (!tracks.length) return [h("h2", { class: "part" }, "Tracks"), emptyState("Not in the library yet. Once it's grabbed and imported, its tracks show here.", null)];
  const discs = new Map();
  for (const track of tracks) discs.set(track.disc_number ?? 1, [...(discs.get(track.disc_number ?? 1) ?? []), track]);
  const cell = (label, content, cls) => h("td", { "data-label": label, class: cls }, content);
  const format = (t) => [t.codec?.toUpperCase(), t.bit_depth ? `${t.bit_depth}-bit` : null, t.sample_rate ? `${t.sample_rate / 1000} kHz` : null, t.bitrate_kbps ? `${t.bitrate_kbps} kbps` : null].filter(Boolean).join(", ");
  return [
    h("h2", { class: "part" }, "Tracks"),
    [...discs.entries()].map(([disc, list]) => [
      discs.size > 1 ? h("h3", { class: "part-sub" }, `Disc ${disc}`) : null,
      h(
        "div",
        { class: "table-wrap stacks" },
        h(
          "table",
          { class: "data" },
          h("caption", { class: "visually-hidden" }, `Disc ${disc}`),
          h("thead", {}, h("tr", {}, ["Track", "Length", "Format"].map((c) => h("th", { scope: "col" }, c)))),
          h("tbody", {}, list.map((t) => h("tr", {}, h("th", { scope: "row" }, `${t.track_number ?? "–"}. ${t.title}`), cell("Length", clock(t.duration_seconds ?? 0)), cell("Format", format(t) || "—")))),
        ),
      ),
    ]),
  ];
}

// What happened to this album: the artist's history, its own events.
function albumHistory(v, title) {
  const box = h("div", {});
  const of = `${title} by `;
  api(`/manage/artists/history?wanted_id=${v.artist.wanted_id}&limit=200`)
    .then((page) => {
      if (!v.ctx.current()) return;
      const mine = page.items.filter((e) => e.title.startsWith(of));
      box.replaceChildren(
        mine.length
          ? h(
              "div",
              { class: "table-wrap stacks" },
              h("table", { class: "data" }, h("caption", { class: "visually-hidden" }, "History, newest first"), h("thead", {}, h("tr", {}, ["What happened", "When", "What", "Word on it"].map((c) => h("th", { scope: "col" }, c)))), h("tbody", {}, mine.map((e) => eventRow(e)))),
            )
          : emptyState("Nothing has happened to it yet: searches, grabs and imports show here.", null),
      );
    })
    .catch((err) => {
      if (err instanceof SignedOut) return v.ctx.signedOut();
      box.replaceChildren(failed("its history", err, null));
    });
  return [h("h2", { class: "part" }, "History"), box];
}
