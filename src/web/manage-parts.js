// Pieces Manage's pages are built from. For the Indexers and System pages (styles in tools.css):
//
// - attentionStrip(findings, here): what needs attention in a service, from the admin health
//   (health.js), as a slim strip at the top of its first page: each finding in words with the
//   mark of its level, and a link to where it's dealt with unless that's the page itself (`here`,
//   its address). Nothing at all when there's nothing to say.
// - onOff({ on, label, change }): a switch for turning something on or off at once, its state
//   in words beside it. `change(next)` does it, answering false to leave the switch as it was
//   (having said why itself).
// - rowTable(headings, rows): a table whose rows stack into labelled blocks on a phone. Each
//   row is { cells: [...], more } where `more` is an element under the row, the width of the
//   table, for what a row opens (a test's answer, a form, a question).
// - askFirst(where, { question, confirm, keep, run, back }): a question asked in place before
//   something that can't be undone, the safe button focused; `back` is where focus goes when
//   it's not done.
// - ago(unix): how long ago a moment was, in words ("4 minutes ago"), for what happened lately.
//
// For Movies, Series, Anime and Music (styles in manage.css): each service's lists and words
// (KINDS), a title's state in words, and changing several wanted titles at once.

import { confirmInline } from "/web/forms.js";
import { SignedOut, airDate, announce, api, fileName, h, languageName, plural, size } from "/web/lib.js";
import { stateLabel } from "/web/page-parts.js";

const LEVELS = { problem: ["failed", "Problem"], waiting: ["waiting", "Waiting"], note: ["off", "Note"] };
const ORDER = { problem: 0, waiting: 1, note: 2 };

export function attentionStrip(findings, here) {
  if (!findings?.length) return null;
  const sorted = [...findings].sort((a, b) => ORDER[a.level] - ORDER[b.level]);
  return h(
    "section",
    { class: "attention", "aria-labelledby": "attention-title" },
    h("h2", { id: "attention-title" }, "Needs attention"),
    h(
      "ul",
      {},
      sorted.map((f) => {
        const [state, word] = LEVELS[f.level] ?? LEVELS.note;
        return h(
          "li",
          {},
          stateLabel(state, word),
          h("span", {}, f.text),
          f.href && f.href !== here ? h("a", { href: f.href }, "Deal with it") : null,
        );
      }),
    ),
  );
}

export function onOff({ on, label, change }) {
  const words = h("span", { class: "switch-words" }, on ? "On" : "Off");
  const button = h("button", { type: "button", role: "switch", class: "switch", "aria-checked": String(on), "aria-label": label }, h("span", { class: "switch-track", "aria-hidden": "true" }));
  button.addEventListener("click", async () => {
    const next = button.getAttribute("aria-checked") !== "true";
    button.disabled = true;
    const done = await change(next);
    button.disabled = false;
    if (done === false) return;
    button.setAttribute("aria-checked", String(next));
    words.textContent = next ? "On" : "Off";
  });
  return h("span", { class: "switch-box" }, button, words);
}

export function rowTable(headings, rows) {
  return h(
    "div",
    { class: "table-wrap" },
    h(
      "table",
      { class: "data stacks" },
      h("thead", {}, h("tr", {}, headings.map((name) => h("th", { scope: "col" }, name)))),
      rows.map((row) =>
        h(
          "tbody",
          {},
          h(
            "tr",
            {},
            row.cells.map((cell, i) => (i === 0 ? h("th", { scope: "row" }, cell) : h("td", { "data-label": headings[i] || null }, cell))),
          ),
          row.more ? h("tr", { class: "row-more" }, h("td", { colspan: headings.length }, row.more)) : null,
        ),
      ),
    ),
  );
}

const relative = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
const STEPS = [
  [60, "second"],
  [60, "minute"],
  [24, "hour"],
  [7, "day"],
  [Infinity, "week"],
];

export function ago(unix) {
  if (!unix) return "never";
  let n = Math.round(unix - Date.now() / 1000);
  for (const [size, unit] of STEPS) {
    if (Math.abs(n) < size || unit === "week") return relative.format(n, unit);
    n = Math.round(n / size);
  }
  return "";
}

export function askFirst(where, { question, confirm, keep = "Cancel", run, back, signedOut }) {
  const go = h("button", { type: "button", class: "danger" }, confirm);
  const cancel = h("button", { type: "button", class: "quiet" }, keep);
  const said = h("p", { class: "error", role: "alert", hidden: true });
  const close = () => {
    where.replaceChildren();
    back?.focus();
  };
  cancel.addEventListener("click", close);
  go.addEventListener("click", async () => {
    go.disabled = cancel.disabled = true;
    try {
      await run();
    } catch (err) {
      if (err instanceof SignedOut) return signedOut?.();
      said.textContent = err.message;
      said.hidden = false;
      go.disabled = cancel.disabled = false;
      cancel.focus();
    }
  });
  where.replaceChildren(
    h("div", { class: "ask", role: "group", "aria-label": confirm }, h("p", {}, question), h("div", { class: "form-actions" }, go, cancel), said),
  );
  where.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && where.contains(cancel)) close();
  });
  cancel.focus();
}

// ---------- Movies, Series, Anime and Music ----------

// Each service's managed list, its wanted list, the profiles it's decided with, what "search
// everything missing" calls it, and its titles' words.
export const KINDS = {
  movies: {
    list: "/manage/movies",
    wanted: "/wanted/movies",
    profiles: { path: "/profiles", service: "movies" },
    area: "movies",
    upgrades: true,
    one: "movie",
    many: "movies",
    watch: (t) => (t.movie_id ? `#/movies/${t.movie_id}` : null),
  },
  series: {
    list: "/manage/series",
    wanted: "/wanted/series",
    profiles: { path: "/profiles", service: "series" },
    area: "series",
    upgrades: true,
    one: "series",
    many: "series",
    watch: (t) => (t.series_id ? `#/series/${t.series_id}` : null),
  },
  anime: {
    list: "/manage/anime",
    wanted: "/wanted/series",
    profiles: { path: "/profiles", service: "anime" },
    area: "series",
    upgrades: true,
    one: "series",
    many: "series",
    watch: (t) => (t.series_id ? `#/series/${t.series_id}` : null),
  },
  music: {
    list: "/manage/artists",
    wanted: "/wanted/artists",
    profiles: { path: "/music/profiles" },
    area: "albums",
    upgrades: false,
    one: "artist",
    many: "artists",
    watch: (t) => (t.artist_id ? `#/music/artists/${t.artist_id}` : null),
  },
};

// The profiles a service's titles can be given: [{ id, name }]. Series and Anime each have their own.
export async function profilesFor(kind) {
  const all = await api(kind.profiles.path);
  return all.filter((p) => !kind.profiles.service || p.service === kind.profiles.service);
}

// A wanted title's profile: its name, or for one with none (approved with nothing to choose
// it), says so with a picker to choose one there; `short` says it in two words, for a table.
// `done(message)` draws it again.
export function profileOrPicker(ctx, kind, t, profiles, done, short = false) {
  if (!t.wanted || t.profile || !kind.profiles.service) return t.profile ?? "—";
  const none = h("span", {}, short ? "None chosen" : "None chosen: nothing is grabbed until one is.");
  if (!profiles.length) return none;
  const said = h("span", { class: "note", role: "status" });
  const pick = h("select", { class: "compact", "aria-label": `Profile for ${t.title}` }, h("option", { value: "" }, "Choose a profile"), profiles.map((p) => h("option", { value: String(p.id) }, p.name)));
  const set = h("button", { type: "submit", class: "quiet" }, "Set profile");
  const form = h("form", { class: "profile-pick", novalidate: true }, none, " ", pick, " ", set, " ", said);
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    if (!pick.value) {
      said.textContent = "Choose one first.";
      return pick.focus();
    }
    const name = pick.selectedOptions[0].textContent;
    set.disabled = pick.disabled = true;
    said.textContent = "Saving…";
    try {
      await api(`${kind.wanted}/${t.wanted_id}/profile`, { method: "PUT", body: { profile_id: Number(pick.value) } });
      const message = `Set ${name} on ${t.title}.`;
      announce(message);
      done(message);
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      said.textContent = err.message;
      set.disabled = pick.disabled = false;
    }
  });
  return form;
}

const STEP = { downloading: "Downloading", transferring: "Copying home", importing: "Placing in the library" };

// A title's state for the key's marks, and its words: the file's quality, how far a download
// has got, when it comes out, or how much of a series or an artist is here.
export function titleState(t) {
  const parts = t.episodes_monitored !== undefined ? episodesWords(t) : t.albums_wanted !== undefined ? albumsWords(t) : null;
  switch (t.state) {
    case "downloading": {
      const step = STEP[t.download?.step] ?? "Downloading";
      const percent = t.download?.step === "downloading" && t.download.percent !== null ? ` ${Math.round(t.download.percent)}%` : "";
      return { mark: "coming", words: `${step}${percent}` };
    }
    case "missing":
      return { mark: "missing", words: parts ?? "Missing" };
    case "unreleased":
      return { mark: "notyet", words: t.release_date ? `Out ${airDate(t.release_date)}` : "Not out yet" };
    case "unmonitored":
      return { mark: "off", words: "Not monitored" };
    default:
      return { mark: "have", words: parts ?? t.quality ?? "In the library" };
  }
}

function episodesWords(t) {
  if (t.episodes_monitored === null) return t.episodes_on_disk ? plural(t.episodes_on_disk, "episode", "episodes") : null;
  return `${t.episodes_on_disk} of ${plural(t.episodes_monitored, "episode", "episodes")}`;
}

function albumsWords(t) {
  if (!t.wanted) return plural(t.albums_on_disk, "album", "albums");
  return `${t.albums_on_disk} of ${plural(t.albums_wanted, "album", "albums")}`;
}

export function stateOf(t) {
  const { mark, words } = titleState(t);
  return stateLabel(mark, words);
}

// The address of a title's page in Manage: by its wanted entry, else by its folder ("f12").
export function titleHref(service, t) {
  const page = service === "music" ? "artist" : "title";
  if (t.wanted_id !== null) return `#/admin/${service}/${page}/${t.wanted_id}`;
  const folder = t.movie_id ?? t.series_id ?? t.artist_id;
  return folder ? `#/admin/${service}/${page}/f${folder}` : null;
}

// A monitoring switch: a button that says whether it's on, in words (`shown`, or only to screen
// readers through `label`). `change(on)` is awaited; a refusal turns it back and says why.
export function monitorSwitch({ ctx, label, on, shown = false, change, said }) {
  const words = h("span", { class: shown ? null : "visually-hidden" });
  const button = h("button", { type: "button", role: "switch", class: "switch", "aria-label": shown ? null : label }, h("span", { class: "switch-track", "aria-hidden": "true" }), words);
  const draw = (value) => {
    button.setAttribute("aria-checked", String(value));
    words.textContent = value ? "Monitored" : "Not monitored";
  };
  draw(on);
  button.addEventListener("click", async () => {
    const next = button.getAttribute("aria-checked") !== "true";
    button.disabled = true;
    draw(next);
    try {
      await change(next);
      announce(`${label}: ${next ? "monitored" : "not monitored"}`);
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      draw(!next);
      if (said) said.textContent = err.message;
      announce(err.message);
    } finally {
      button.disabled = false;
    }
  });
  return button;
}

// Changing several wanted titles at once, one call each, saying what happened to each that
// failed. `titles` are managed-list rows on the wanted list.
export async function changeEach(titles, change) {
  const failed = [];
  for (const t of titles) {
    try {
      await change(t);
    } catch (err) {
      if (err instanceof SignedOut) throw err;
      failed.push(`${t.title}: ${err.message}`);
    }
  }
  return failed;
}

// The bar under the toolbar while titles are selected: monitor or stop, set the profile, take
// off the list (asked first, saying files stay). `done(message)` draws the list again.
export function selectionBar(ctx, kind, { selected, profiles, done }) {
  const count = h("p", { class: "selected-count", role: "status" });
  const actions = h("div", { class: "row-actions" });
  // `said(titles)` says what was done to them, as a sentence.
  const run = async (said, change) => {
    const titles = selected();
    if (!titles.length) return;
    for (const b of actions.querySelectorAll("button, select")) b.disabled = true;
    try {
      const failed = await changeEach(titles, change);
      const ok = titles.length - failed.length;
      const message = `${said(plural(ok, kind.one, kind.many))}${failed.length ? ` Not changed: ${failed.join("; ")}.` : ""}`;
      announce(message);
      done(message);
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      throw err;
    }
  };
  const monitor = (on) => (t) => api(`${kind.wanted}/${t.wanted_id}/monitored`, { method: "PUT", body: { monitored: on } });
  const profile = h("select", { class: "compact", "aria-label": "Profile" }, h("option", { value: "" }, "Profile…"), profiles.map((p) => h("option", { value: String(p.id) }, p.name)));
  const setProfile = h("button", { type: "button", class: "quiet" }, "Set profile");
  setProfile.addEventListener("click", () => {
    if (!profile.value) return profile.focus();
    const name = profile.selectedOptions[0].textContent;
    run((n) => `Set ${name} on ${n}.`, (t) => api(`${kind.wanted}/${t.wanted_id}/profile`, { method: "PUT", body: { profile_id: Number(profile.value) } }));
  });
  const takeOff = h("button", { type: "button", class: "quiet" }, "Take off the list…");
  takeOff.addEventListener("click", () => {
    const n = selected().length;
    if (!n) return;
    const them = n === 1 ? ["it", "its"] : ["them", "their"];
    confirmInline(actions, {
      question: `Take ${plural(n, kind.one, kind.many)} off the wanted list? Melete stops looking for ${them[0]}; ${them[1]} files in the library stay, and ${them[0]} can be added again.`,
      confirm: "Take off",
      run: () => run((n) => `Took ${n} off the wanted list.`, (t) => api(`${kind.wanted}/${t.wanted_id}`, { method: "DELETE" })),
    });
  });
  const parts = [
    h("button", { type: "button", class: "quiet", onclick: () => run((n) => `Monitoring ${n}.`, monitor(true)) }, "Monitor"),
    h("button", { type: "button", class: "quiet", onclick: () => run((n) => `Stopped monitoring ${n}.`, monitor(false)) }, "Stop monitoring"),
    profiles.length ? [profile, setProfile] : null,
    takeOff,
  ];
  const bar = h("div", { class: "selection-bar" }, count, actions);
  return {
    bar,
    update() {
      const n = selected().length;
      count.textContent = n ? `${plural(n, kind.one, kind.many)} selected` : `Select ${kind.many} on the wanted list to change them.`;
      if (!actions.querySelector(".question")) actions.replaceChildren(...parts.flat().filter(Boolean));
      for (const b of actions.querySelectorAll("button, select")) b.disabled = n === 0;
    },
  };
}

// A row per file with what ffprobe found in it; one not read yet says so.
export async function fileRows(files) {
  const read = await Promise.all(files.map((f) => api(`/files/${f.id}`).catch(() => null)));
  const cell = (label, content, cls) => h("td", { "data-label": label, class: cls }, content);
  return h(
    "div",
    { class: "table-wrap stacks" },
    h(
      "table",
      { class: "data" },
      h("caption", { class: "visually-hidden" }, "Files"),
      h("thead", {}, h("tr", {}, ["File", "Video", "Audio", "Subtitles", "Size"].map((c) => h("th", { scope: "col", class: c === "Size" ? "r" : null }, c)))),
      h(
        "tbody",
        {},
        files.map((f, i) => {
          const m = read[i]?.media;
          return h(
            "tr",
            {},
            h("th", { scope: "row" }, h("span", { class: "path" }, fileName(f.path))),
            cell("Video", m ? videoWords(m) : "Not read yet"),
            cell("Audio", m ? tracks(m.audio, (a) => [languageName(a.language), a.channel_layout ?? (a.channels ? `${a.channels} ch` : null), a.codec].filter(Boolean).join(" ")) : "—"),
            cell("Subtitles", m ? tracks(m.subtitles, (s) => languageName(s.language) ?? s.codec) : "—"),
            cell("Size", size(f.size), "r"),
          );
        }),
      ),
    ),
  );
}

function videoWords(m) {
  const video = m.video[0];
  if (!video) return "None";
  return [video.codec.toUpperCase(), video.height ? `${video.height}p` : null, video.hdr ?? null].filter(Boolean).join(", ");
}

function tracks(list, words) {
  if (!list?.length) return "None";
  return [...new Set(list.map(words))].join("; ");
}
