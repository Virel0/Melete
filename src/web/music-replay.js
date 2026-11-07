// Replay: this listener's own listening this month, this year or all of it, as the Aoide apps'
// Replay shows it (`GET /music/replay`): plays, time, songs, artists and albums, the busiest
// day, and the top songs (which play), artists and albums; then every listen of the period,
// newest first (`GET /music/plays`), each of which can be deleted, or the whole period at once.
// Deleting is from Melete's history only: Aoide devices keep a listen they already have.
//
// The period is in the address (`?period=year`), so Back returns to it.
//
// For album and artist pages: yourListening(query) gives a line saying how often this listener
// played them and how often they finish them, and artistTopSongs(id, ctx) their top songs by
// that artist; both fill in once Replay answers, and say nothing when there's nothing to say.

import { confirmInline, when } from "/web/forms.js";
import { SignedOut, announce, api, h, number, plural, show } from "/web/lib.js";
import { trackActions } from "/web/music-actions.js";
import { albumCard, countedItem, markPlaying, songRow } from "/web/music-parts.js";
import { looseQueue, play, shufflePlay } from "/web/music-play.js";
import { emptyState, failed, placeholders } from "/web/page-parts.js";

const PERIODS = [
  ["month", "This month"],
  ["year", "This year"],
  ["all", "All time"],
];
// Listens shown at a time in the history.
const PAGE = 50;

// Where a period starts, on this browser's clock, in Unix seconds; none for all time.
function start(period, now = new Date()) {
  if (period === "month") return Math.floor(new Date(now.getFullYear(), now.getMonth(), 1) / 1000);
  if (period === "year") return Math.floor(new Date(now.getFullYear(), 0, 1) / 1000);
  return null;
}

// "3 h 20 min", "45 min".
function duration(minutes) {
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  if (!hours) return `${number.format(rest)} min`;
  return rest ? `${number.format(hours)} h ${rest} min` : `${number.format(hours)} h`;
}

const dayFormat = new Intl.DateTimeFormat(undefined, { weekday: "long", day: "numeric", month: "long", year: "numeric" });

export async function replayPage(params, ctx) {
  const period = PERIODS.some(([key]) => key === params.get("period")) ? params.get("period") : "month";
  const label = PERIODS.find(([key]) => key === period)[1];
  const title = h("h1", {}, "Replay");
  const tabs = h(
    "nav",
    { class: "tabs", "aria-label": "Period" },
    PERIODS.map(([key, name]) => h("a", { href: `#/music/replay?period=${key}`, "aria-current": key === period ? "page" : null }, name)),
  );
  show("Replay", title, tabs, placeholders("your listening"));
  const from = start(period);
  const query = new URLSearchParams({ offset_minutes: String(-new Date().getTimezoneOffset()) });
  if (from !== null) query.set("from", String(from));
  let replay;
  try {
    replay = await api(`/music/replay?${query}`);
  } catch (err) {
    if (err instanceof SignedOut) throw err;
    if (ctx.current()) show("Replay", title, tabs, failed("your Replay", err, () => replayPage(params, ctx)));
    return;
  }
  if (!ctx.current()) return;
  if (!replay.plays && !replay.minutes) {
    show("Replay", title, tabs, emptyState(`Nothing played ${period === "all" ? "yet" : label.toLowerCase()}. Songs you play here or on your Aoide devices count.`, { href: "#/music", label: "Go to Listen" }));
    return;
  }
  const songs = topSongs(replay.top_songs);
  show(
    "Replay",
    title,
    tabs,
    summary(replay),
    songs,
    topArtists(replay.top_artists),
    topAlbums(replay.top_albums),
    history(period, from, label, ctx, () => replayPage(params, ctx)),
  );
  if (songs) markPlaying(songs, ctx);
}

// The period in figures: plays, time, songs, artists, albums, and the busiest day.
function summary(replay) {
  const figures = [
    [number.format(replay.plays), replay.plays === 1 ? "play" : "plays"],
    [duration(replay.minutes), "listened"],
    [number.format(replay.songs), replay.songs === 1 ? "song" : "songs"],
    [number.format(replay.artists), replay.artists === 1 ? "artist" : "artists"],
    [number.format(replay.albums), replay.albums === 1 ? "album" : "albums"],
  ];
  const busiest = replay.busiest_day;
  return h(
    "section",
    { class: "replay-summary", "aria-label": "In figures" },
    h("dl", { class: "replay-figures" }, figures.map(([value, what]) => h("div", {}, h("dt", {}, what), h("dd", {}, value)))),
    busiest
      ? h("p", { class: "status" }, `Your busiest day: ${dayFormat.format(new Date(`${busiest.date}T12:00:00`))}, with ${plural(busiest.plays, "play", "plays")}.`)
      : null,
  );
}

function topSongs(top) {
  if (!top.length) return null;
  const tracks = top.map((t) => t.track);
  const queue = looseQueue(tracks);
  const list = h(
    "ol",
    { class: "tracks songs replay-songs" },
    top.map((t, i) => {
      const row = songRow(t.track, () => play(queue, i), trackActions(t.track, queue[i]));
      row.querySelector(".length")?.replaceChildren(plural(t.plays, "play", "plays"));
      return row;
    }),
  );
  return h(
    "section",
    { "aria-labelledby": "replay-songs" },
    h("div", { class: "section-head" }, h("h2", { id: "replay-songs" }, "Top songs")),
    h(
      "div",
      { class: "play-actions" },
      h("button", { type: "button", onclick: () => play(queue) }, "Play"),
      h("button", { type: "button", class: "quiet", onclick: () => shufflePlay(queue) }, "Shuffle"),
    ),
    list,
  );
}

function topArtists(top) {
  if (!top.length) return null;
  return h(
    "section",
    { "aria-labelledby": "replay-artists" },
    h("h2", { id: "replay-artists" }, "Top artists"),
    h(
      "ol",
      { class: "artist-list replay-list" },
      top.map((a) => countedItem(`#/music/artists/${a.artist.id}`, a.artist.name, plural(a.plays, "play", "plays"))),
    ),
  );
}

function topAlbums(top) {
  if (!top.length) return null;
  return h(
    "section",
    { "aria-labelledby": "replay-albums" },
    h("h2", { id: "replay-albums" }, "Top albums"),
    h(
      "ul",
      { class: "grid" },
      top.map((a) => {
        const card = albumCard(a.album);
        card.querySelector(".card")?.append(h("div", { class: "meta" }, plural(a.plays, "play", "plays")));
        return card;
      }),
    ),
  );
}

// "Finished", "Skipped at 0:31", "Stopped at 2:05", or nothing while it plays.
function outcome(listen) {
  if (listen.completed) return "Finished";
  const at = listen.position_seconds;
  const clock = at == null ? "" : ` at ${Math.floor(at / 60)}:${String(Math.floor(at % 60)).padStart(2, "0")}`;
  if (listen.skipped) return `Skipped${clock}`;
  return listen.ended_at ? `Stopped${clock}` : "";
}

// Every listen of the period, newest first, a page at a time, each deletable; and deleting the
// whole period.
function history(period, from, label, ctx, redraw) {
  const list = h("ol", { class: "replay-history" });
  const more = h("div", { class: "row-actions" });
  const said = h("p", { class: "status", role: "status" });
  const tools = h("div", { class: "row-actions" });
  let offset = 0;
  const load = async () => {
    const query = new URLSearchParams({ limit: String(PAGE), offset: String(offset) });
    if (from !== null) query.set("since", String(from));
    let page;
    try {
      page = await api(`/music/plays?${query}`);
    } catch (err) {
      if (err instanceof SignedOut) throw err;
      more.replaceChildren(failed("your listens", err, load));
      return;
    }
    if (!ctx.current()) return;
    offset += page.items.length;
    list.append(...page.items.map((listen) => listenRow(listen)));
    said.textContent = `${plural(page.total, "listen", "listens")} ${period === "all" ? "in all" : label.toLowerCase()}`;
    more.replaceChildren(
      offset < page.total ? h("button", { type: "button", class: "quiet", onclick: () => load() }, `Show ${Math.min(PAGE, page.total - offset)} more`) : "",
    );
  };
  const clear = h("button", { type: "button", class: "quiet" }, `Delete listens from ${period === "all" ? "all time" : label.toLowerCase()}`);
  clear.addEventListener("click", () =>
    confirmInline(tools, {
      question: `Delete every listen ${period === "all" ? "you have" : `from ${label.toLowerCase()}`} from Melete's history? Replay and your mixes stop counting them. Your Aoide devices keep the ones they already have. This can't be undone.`,
      confirm: "Delete listens",
      run: async () => {
        const query = from === null ? "all=true" : `from=${from}`;
        const done = await api(`/music/plays?${query}`, { method: "DELETE" });
        announce(`${plural(done.deleted, "listen", "listens")} deleted.`);
        await redraw();
      },
    }),
  );
  tools.append(clear);
  load();
  return h(
    "section",
    { "aria-labelledby": "replay-history" },
    h("h2", { id: "replay-history" }, "Every listen"),
    said,
    list,
    more,
    tools,
  );
}

function listenRow(listen) {
  const track = listen.track;
  const name = track ? track.title : "A song no longer in the library";
  const by = track ? [track.artist ?? track.album_artist, track.album].filter(Boolean).join(", ") : "";
  const actions = h("div", { class: "row-actions" });
  const remove = h("button", { type: "button", class: "quiet", "aria-label": `Delete this listen of ${name}` }, "Delete");
  remove.addEventListener("click", () =>
    confirmInline(actions, {
      question: "Delete this listen from Melete's history?",
      confirm: "Delete",
      run: async () => {
        await api(`/music/plays/${listen.id}`, { method: "DELETE" });
        row.remove();
        announce(`The listen of ${name} was deleted.`);
      },
    }),
  );
  actions.append(remove);
  const row = h(
    "li",
    {},
    h("span", { class: "name" }, h("span", {}, name), by ? h("span", { class: "by" }, by) : null),
    h("span", { class: "when" }, when(listen.started_at), listen.device ? ` · ${listen.device}` : ""),
    h("span", { class: "outcome" }, outcome(listen)),
    actions,
  );
  return row;
}

// The apps show a finish rate from this many decided listens.
const FINISH_SAMPLE = 3;

// "You've played it 12 times and finish 80% of it", or less, or nothing; filled in later.
export function yourListening(query, what) {
  const line = h("p", { class: "status your-listening" });
  api(`/music/replay?${query}&limit=1`)
    .then((replay) => {
      const { decided, finished } = replay.finish;
      const rate = decided >= FINISH_SAMPLE ? `${Math.round((100 * finished) / decided)}%` : null;
      const played = replay.plays ? `You've played ${what} ${plural(replay.plays, "time", "times")}` : null;
      if (played && rate) line.textContent = `${played}, and you finish ${rate} of what you start.`;
      else if (played) line.textContent = `${played}.`;
      else if (rate) line.textContent = `You finish ${rate} of what you start of ${what}.`;
    })
    .catch(() => {});
  return line;
}

// This listener's five most played songs by an artist, which play; nothing until they have some.
export function artistTopSongs(artistId, ctx) {
  const section = h("section", { class: "artist-top-songs", "aria-labelledby": "artist-top-songs", hidden: true });
  api(`/music/replay?artist_id=${artistId}&limit=5`)
    .then((replay) => {
      if (!ctx.current() || !replay.top_songs.length) return;
      const tracks = replay.top_songs.map((t) => t.track);
      const queue = looseQueue(tracks);
      const list = h(
        "ol",
        { class: "tracks songs replay-songs" },
        replay.top_songs.map((t, i) => {
          const row = songRow(t.track, () => play(queue, i), trackActions(t.track, queue[i]));
          row.querySelector(".length")?.replaceChildren(plural(t.plays, "play", "plays"));
          return row;
        }),
      );
      section.append(h("h2", { id: "artist-top-songs" }, "Your top songs"), list);
      section.hidden = false;
      markPlaying(list, ctx);
    })
    .catch(() => {});
  return section;
}
