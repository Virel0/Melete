// The calendar: what the household is waiting for, by day, as Radarr's and Sonarr's
// calendars show it. Episodes of wanted series on their air dates, and wanted movies on their
// cinema, digital and disc release dates, each saying whether it's in the library, downloading,
// missing or still to come. An agenda (a list of days) or a month; the view, the dates and the
// filter stay in the address, so Back and links return to the same place.

import { SignedOut, api, failure, h, loading, show } from "/web/lib.js";

const VIEWS = [
  ["agenda", "Agenda"],
  ["month", "Month"],
];
const SHOWS = [
  ["all", "Everything"],
  ["movies", "Movies"],
  ["episodes", "Episodes"],
];
// The agenda's span, inside the API's 90 days.
const AGENDA_DAYS = 30;
const RELEASES = { cinema: "In cinemas", digital: "Digital release", physical: "Disc release" };

// Dates as the API has them ("2026-10-27"), worked out in local time.
const iso = (d) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
const parse = (text) => {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(text ?? "");
  return m ? new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3])) : null;
};
const addDays = (d, n) => new Date(d.getFullYear(), d.getMonth(), d.getDate() + n);

// The days a view covers, and where Previous and Next go.
function span(view, from) {
  if (view === "month") {
    const first = new Date(from.getFullYear(), from.getMonth(), 1);
    // Weeks start on Monday; the grid runs from the Monday on or before the 1st to the Sunday
    // on or after the month's last day.
    const start = addDays(first, -((first.getDay() + 6) % 7));
    const last = new Date(from.getFullYear(), from.getMonth() + 1, 0);
    const end = addDays(last, (7 - last.getDay()) % 7);
    return {
      start,
      end,
      previous: new Date(from.getFullYear(), from.getMonth() - 1, 1),
      next: new Date(from.getFullYear(), from.getMonth() + 1, 1),
      label: first.toLocaleDateString(undefined, { month: "long", year: "numeric" }),
    };
  }
  const end = addDays(from, AGENDA_DAYS - 1);
  const fmt = (d) => d.toLocaleDateString(undefined, { day: "numeric", month: "short" });
  return { start: from, end, previous: addDays(from, -AGENDA_DAYS), next: addDays(from, AGENDA_DAYS), label: `${fmt(from)} to ${fmt(end)}` };
}

// Where an entry stands, in words, with a class for its badge.
function status(e, today) {
  if (e.has_file) return ["In the library", "state-done"];
  if (e.download_state) return ["Downloading", "state-active"];
  if (!e.monitored) return ["Not monitored", "state-quiet"];
  return e.date < today ? ["Missing", "state-failed"] : ["Coming", "state-quiet"];
}

// Where an entry leads: its title in Watch, unless the page says otherwise (`opts.href`).
const watchHref = (e) => (e.movie_id ? `#/movies/${e.movie_id}` : e.series_id ? `#/series/${e.series_id}?season=${e.season}` : null);

function entryItem(e, today, opts) {
  const isEpisode = e.kind === "episode";
  const name = isEpisode ? e.title : `${e.title}${e.year ? ` (${e.year})` : ""}`;
  const href = (opts.href ?? watchHref)(e);
  const what = isEpisode
    ? `S${String(e.season).padStart(2, "0")}E${String(e.episode).padStart(2, "0")}${e.name ? `: ${e.name}` : ""}`
    : RELEASES[e.kind] ?? e.kind;
  const [word, cls] = status(e, today);
  return h(
    "li",
    { class: "calendar-entry" },
    href ? h("a", { href, class: "title" }, name) : h("span", { class: "title" }, name),
    h("span", { class: "what" }, what),
    h("span", { class: `badge ${cls}` }, word),
  );
}

function agenda(days, entries, today, opts) {
  const byDay = new Map();
  for (const e of entries) {
    if (!byDay.has(e.date)) byDay.set(e.date, []);
    byDay.get(e.date).push(e);
  }
  const shown = days.filter((d) => byDay.has(d));
  if (!shown.length) return h("p", { class: "status" }, "Nothing in these days.");
  return h(
    "div",
    { class: "calendar-agenda" },
    shown.map((d) =>
      h(
        "section",
        { class: d === today ? "calendar-day today" : "calendar-day" },
        h("h2", {}, parse(d).toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "long" }), d === today ? h("span", { class: "badge" }, "Today") : null),
        h("ul", { class: "calendar-entries" }, byDay.get(d).map((e) => entryItem(e, today, opts))),
      ),
    ),
  );
}

function month(days, entries, today, from, opts) {
  const byDay = new Map(days.map((d) => [d, []]));
  for (const e of entries) byDay.get(e.date)?.push(e);
  const weekdays = days.slice(0, 7).map((d) => parse(d).toLocaleDateString(undefined, { weekday: "short" }));
  const rows = [];
  for (let i = 0; i < days.length; i += 7) {
    rows.push(
      h(
        "tr",
        {},
        days.slice(i, i + 7).map((d) => {
          const date = parse(d);
          const classes = ["calendar-cell", date.getMonth() !== from.getMonth() ? "other-month" : "", d === today ? "today" : ""].filter(Boolean).join(" ");
          const list = byDay.get(d);
          return h(
            "td",
            { class: classes },
            h("div", { class: "calendar-date", "aria-current": d === today ? "date" : null }, String(date.getDate())),
            list.length ? h("ul", { class: "calendar-entries" }, list.map((e) => entryItem(e, today, opts))) : null,
          );
        }),
      ),
    );
  }
  return h(
    "div",
    { class: "calendar-month-wrap" },
    h("table", { class: "calendar-month" }, h("thead", {}, h("tr", {}, weekdays.map((w) => h("th", { scope: "col" }, w)))), h("tbody", {}, rows)),
  );
}

// `opts` lets a Manage service show its own part of it: `base` (its address), `keep(entry)`,
// `shows` (false leaves out the Everything, Movies, Episodes choice), `href(entry)`, and
// `render(title, ...nodes)` in place of drawing the page.
export async function calendarPage(params, ctx, opts = {}) {
  const { base = "#/calendar", keep = () => true, shows: choose = true, render = show } = opts;
  const view = VIEWS.some(([k]) => k === params.get("view")) ? params.get("view") : "agenda";
  const shows = SHOWS.some(([k]) => k === params.get("show")) ? params.get("show") : "all";
  const now = new Date();
  const todayDate = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const today = iso(todayDate);
  // The agenda starts yesterday by default, so last night's episodes are still there.
  const from = parse(params.get("from")) ?? (view === "month" ? todayDate : addDays(todayDate, -1));
  const { start, end, previous, next, label } = span(view, from);
  const link = (changes) => {
    const p = new URLSearchParams({ view, show: shows, from: iso(from), ...changes });
    return `${base}?${p}`;
  };

  const toolbar = h(
    "div",
    { class: "calendar-toolbar" },
    h(
      "nav",
      { "aria-label": "Dates" },
      h("a", { class: "button quiet", href: link({ from: iso(previous) }) }, "Previous"),
      h("a", { class: "button quiet", href: link({ from: iso(view === "month" ? todayDate : addDays(todayDate, -1)) }) }, "Today"),
      h("a", { class: "button quiet", href: link({ from: iso(next) }) }, "Next"),
    ),
    h(
      "nav",
      { "aria-label": "View" },
      VIEWS.map(([k, name]) => h("a", { class: "button quiet", href: link({ view: k, from: iso(k === "month" ? todayDate : addDays(todayDate, -1)) }), "aria-current": k === view ? "page" : null }, name)),
    ),
    choose
      ? h(
          "nav",
          { "aria-label": "Show" },
          SHOWS.map(([k, name]) => h("a", { class: "button quiet", href: link({ show: k }), "aria-current": k === shows ? "page" : null }, name)),
        )
      : null,
  );
  const heading = [h("h1", {}, "Calendar"), h("p", { class: "status" }, label), toolbar];

  render("Calendar", heading, loading("the calendar"));
  let answer;
  try {
    answer = await api(`/calendar?start=${iso(start)}&end=${iso(end)}`);
  } catch (err) {
    if (err instanceof SignedOut) return ctx.signedOut();
    if (!ctx.current()) return;
    return render("Calendar", heading, failure(err, () => calendarPage(params, ctx, opts)));
  }
  if (!ctx.current()) return;
  const entries = answer.entries.filter((e) => keep(e) && (shows === "all" || (shows === "episodes") === (e.kind === "episode")));
  const days = [];
  for (let d = start; d <= end; d = addDays(d, 1)) days.push(iso(d));
  const body = view === "month" ? month(days, entries, today, from, opts) : agenda(days, entries, today, opts);
  const empty = answer.entries.length === 0
    ? h("p", { class: "note" }, "The calendar shows what's wanted: movies and series you or an admin asked for, by the dates they air or come out.")
    : null;
  render("Calendar", heading, body, empty);
}
