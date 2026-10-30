// Manage → System: Tasks (what runs in the background, how it went, and running one now),
// Backups (the database's copies, their ages and how going back works) and Downloading
// (Deluge and the seedbox, set up here; Soulseek is under Indexers).

import { group, when } from "/web/forms.js";
import { SignedOut, announce, api, h, size } from "/web/lib.js";
import { currentHealth } from "/web/health.js";
import { emptyState, stateLabel } from "/web/page-parts.js";
import { ago, attentionStrip, rowTable } from "/web/manage-parts.js";
import { serviceSection } from "/web/services.js";

// ---------- Tasks ----------

const TASKS = {
  library: "Library scans",
  "tmdb-matching": "TMDB matching",
  "music-matching": "Music matching",
  "music-analysis": "Music analysis",
  segments: "Intros and credits",
  trickplay: "Seek-bar previews",
  works: "Classical works",
  lyrics: "Lyrics",
  "cover-colours": "Cover colours",
  following: "Followed artists",
  "rss-sync": "RSS sync",
  "import-lists": "Import lists",
  "offline-copies": "Offline copies",
  "gpu-check": "GPU check",
  backup: "Backups",
};

// Tasks run from a page of their own.
const RUN_THERE = {
  "rss-sync": { href: "#/admin/indexers/activity", label: "Go to RSS syncs" },
  "import-lists": { href: "#/admin/movies/sources", label: "Go to import lists" },
  backup: { href: "#/admin/system/backups", label: "Go to Backups" },
};

const sentence = (text) => text.charAt(0).toUpperCase() + text.slice(1);

function taskState(t) {
  if (t.running) return stateLabel("coming", "Running");
  if (!t.active) return stateLabel("off", "Off: its service is off");
  if (t.result?.startsWith("failed")) return stateLabel("failed", sentence(t.result));
  if (t.result) return stateLabel("have", sentence(t.result));
  return h("span", { class: "note" }, "Not run since the server started");
}

// A moment in words; a next run already past is due now (it waits for one before it to end).
const soon = (unix, next = false) => {
  if (!unix) return "—";
  const due = next && unix <= Date.now() / 1000;
  return h("span", { class: "task-time", title: when(unix) }, due ? "Due now" : ago(unix));
};

export async function systemTasks(ctx) {
  const [first, found] = await Promise.all([api("/admin/system/tasks"), currentHealth(true)]);
  if (!ctx.current()) return [];
  // What TMDB matching's runs and setup say needs fixing, above the list.
  const strip = attentionStrip((found.system ?? []).filter((f) => f.code?.startsWith("tmdb_")), "#/admin/system/tasks");
  const table = h("div");
  let polling = false;
  const draw = (tasks) => table.replaceChildren(rowTable(["Task", "State", "Last finished", "Next", ""], tasks.map((t) => taskRow(t, refresh, ctx))));
  // While a task runs, its state is asked again every 5 seconds, until the page is left.
  const refresh = async () => {
    const tasks = await api("/admin/system/tasks");
    if (!ctx.current()) return;
    draw(tasks);
    if (tasks.some((t) => t.running) && !polling) {
      polling = true;
      setTimeout(() => {
        polling = false;
        if (ctx.current()) refresh().catch(() => {});
      }, 5000);
    }
  };
  draw(first);
  if (first.some((t) => t.running)) refresh().catch(() => {});
  return [
    strip,
    h(
      "p",
      { class: "note" },
      "What Melete does in the background, as seen since the server started. A failed run says only that it failed, or for TMDB matching what went wrong in a few plain words; the server's log says why, since the reason can name an address. Runs never overlap.",
    ),
    table,
  ];
}

function taskRow(t, refresh, ctx) {
  const name = TASKS[t.name] ?? t.name;
  const more = h("div", { class: "row-room" });
  let action = null;
  if (RUN_THERE[t.name]) {
    action = h("a", { href: RUN_THERE[t.name].href }, RUN_THERE[t.name].label);
  } else if (t.can_run && t.active && !t.running) {
    action = h("button", { type: "button", class: "quiet", "aria-label": `Run ${name} now`, "data-task": t.name }, "Run now");
    action.addEventListener("click", async () => {
      action.disabled = true;
      try {
        await api(`/admin/system/tasks/${t.name}/run`, { method: "POST" });
        announce(`${name} asked to run`);
        await refresh();
        // Its button, or its name while it runs (the button's gone then).
        (document.querySelector(`[data-task="${t.name}"]`) ?? document.querySelector(`[data-task-name="${t.name}"]`))?.focus();
      } catch (err) {
        if (err instanceof SignedOut) return ctx.signedOut();
        action.disabled = false;
        more.replaceChildren(h("p", { class: "error", role: "alert" }, err.message));
      }
    });
  }
  return {
    cells: [h("span", { tabindex: "-1", "data-task-name": t.name }, name, h("span", { class: "note task-what" }, sentence(t.what))), taskState(t), soon(t.last_finished_at), soon(t.next_at, true), action],
    more,
  };
}

// ---------- Backups ----------

const COPY_KINDS = { daily: "Daily", before_migrating: "Before migrating", by_hand: "By hand", other: "Named otherwise" };

export async function systemBackups(ctx) {
  const [backups, found] = await Promise.all([api("/admin/system/backups"), currentHealth(true)]);
  if (!ctx.current()) return [];
  const { copies, latest, kept } = backups;
  const strip = attentionStrip((found.system ?? []).filter((f) => f.code?.startsWith("backup_")), "#/admin/system/backups");
  const tries = latest.length
    ? h(
        "ul",
        { class: "plain", "aria-label": "The latest try of each kind" },
        latest.map((a) => h("li", {}, stateLabel(a.ok ? "have" : "failed", `${COPY_KINDS[a.kind]}: ${a.ok ? "made" : "failed"} ${ago(a.made_at)}`))),
      )
    : h("p", { class: "note" }, "Nothing tried in the last 90 days.");
  const list = copies.length
    ? rowTable(
        ["Copy", "Kind", "Size", "Made"],
        copies.map((c) => ({ cells: [h("span", { class: "path" }, c.file), COPY_KINDS[c.kind], size(c.bytes), h("span", { title: when(c.made_at) }, ago(c.made_at))] })),
      )
    : emptyState("No copies yet. Melete makes the day's copy at startup and once a day after that.");
  return [
    strip,
    group(
      "Copies",
      h(
        "p",
        { class: "note" },
        `Kept on the server, in the data folder's backups, readable by Melete alone; they never leave it. Melete keeps the daily copies of the last ${kept.daily} days, one a week for ${kept.weekly} weeks before them, and the last ${kept.before_migrating} made before migrating; copies made by hand stay until you remove them.`,
      ),
      tries,
      list,
    ),
    group(
      "Making one and going back",
      h("p", {}, "To make a copy now, run ", h("code", {}, "melete backup"), " on the server (inside the container)."),
      h("p", {}, "Going back to a copy is done on the server, with Melete stopped, so nothing writes to the database meanwhile:"),
      h(
        "ol",
        {},
        h("li", {}, "Stop Melete."),
        h("li", {}, "Run ", h("code", {}, "melete restore"), " to list the copies, newest first (in the container: ", h("code", {}, "docker compose run --rm melete melete restore"), ")."),
        h("li", {}, "Run ", h("code", {}, "melete restore <copy>"), " with the one to go back to. It checks the copy first and refuses one that fails, changing nothing; otherwise it keeps the current database in the backups folder, removes ", h("code", {}, "melete.db-wal"), " and ", h("code", {}, "melete.db-shm"), ", and puts the copy in its place."),
        h("li", {}, "Start Melete. An older copy is brought up to date as it starts, after a copy of its own."),
      ),
      h("p", { class: "note" }, "Whatever changed since the copy was made is lost: watch history, plays, requests and settings saved since."),
    ),
  ];
}

// ---------- Downloading ----------

// Deluge and the seedbox, from Outside services' list: where downloads come from and how they
// come home. Soulseek's settings are under Indexers.
export async function downloading(ctx) {
  const all = await api("/settings/services");
  if (!ctx.current()) return [];
  const deluge = all.find((s) => s.key === "downloads");
  const seedbox = all.find((s) => s.key === "seedbox");
  const slskd = all.find((s) => s.key === "slskd");
  const on = (s, yes, no) => (s?.on ? stateLabel("have", yes) : stateLabel("off", no));
  const facts = h(
    "ul",
    { class: "plain downloading" },
    h("li", {}, on(deluge, "Torrents: grabbed through Deluge on the seedbox", "Torrents: off, nothing is grabbed")),
    h("li", {}, on(seedbox, "Finished torrents are copied home from the seedbox", "Finished torrents stay on the seedbox: copying home is off")),
    h("li", {}, on(slskd, "Soulseek: on.", "Soulseek: off."), " ", h("a", { href: "#/admin/indexers/soulseek" }, "Its settings are under Indexers, Soulseek")),
  );
  return [
    group(
      "How downloads come in",
      facts,
      h(
        "p",
        { class: "note" },
        "Each service's downloads are under its Activity: ",
        h("a", { href: "#/admin/movies/activity" }, "Movies"),
        ", ",
        h("a", { href: "#/admin/series/activity" }, "Series"),
        ", ",
        h("a", { href: "#/admin/music/activity" }, "Music"),
        ". Nothing already in a library is renamed or moved; a file an upgrade replaces goes to the trash for 30 days.",
      ),
    ),
    group(
      "Settings",
      [deluge, seedbox].filter(Boolean).map((s) => serviceSection(ctx, s)),
    ),
  ];
}
