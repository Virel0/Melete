// Manage → System → Status, where "#/admin" opens: every service's health, the room left on
// each disk Melete uses, and a line each for the background tasks, backups, libraries, users,
// outside services and devices' reports, each with where to see more. Each part comes on its
// own, so one that can't be read says so and the rest still show.

import { group } from "/web/forms.js";
import { SignedOut, api, h, plural, size } from "/web/lib.js";
import { AREAS, currentHealth } from "/web/health.js";
import { stateLabel } from "/web/page-parts.js";
import { ago } from "/web/manage-parts.js";

const ORDER = { problem: 0, waiting: 1, note: 2 };
const DAY = 86_400;
const HERE = "#/admin/system/status";

// A promise's value, or the error it failed with (a session that ran out still ends the page).
async function settle(promise) {
  try {
    return { value: await promise };
  } catch (err) {
    if (err instanceof SignedOut) throw err;
    return { err };
  }
}

const couldNot = (what, err) => h("p", { class: "error" }, `Couldn't read ${what}: ${err.message}`);

// How an area stands, in words with the mark of its state.
function areaState(list) {
  const problems = list.filter((f) => f.level === "problem").length;
  if (problems) return stateLabel("failed", plural(problems, "problem", "problems"));
  const waiting = list.filter((f) => f.level === "waiting").reduce((n, f) => n + f.count, 0);
  if (waiting) return stateLabel("waiting", `${waiting} waiting`);
  return stateLabel("have", "Nothing needs attention");
}

function health(found) {
  if (found.err) return group("Health", couldNot("how things are", found.err));
  const areas = AREAS.map((a) => {
    const list = [...(found.value[a.key] ?? [])].sort((x, y) => ORDER[x.level] - ORDER[y.level]);
    return h(
      "li",
      {},
      h("div", { class: "area-line" }, h("a", { class: "area-name", href: `#/admin/${a.key}` }, a.name), areaState(list)),
      list.length
        ? h(
            "ul",
            { class: "plain findings" },
            list.map((f) => h("li", {}, f.text, " ", f.href === HERE ? null : h("a", { href: f.href }, f.level === "note" ? "Open" : "Deal with it"))),
          )
        : null,
    );
  });
  return group("Health", h("ul", { class: "areas" }, areas));
}

const PLACES = { data: "Data folder", transcode: "Transcode folder" };

function disk(places) {
  if (places.err) return group("Disk space", couldNot("the disks", places.err));
  const rows = places.value.map((p) => {
    const name = p.kind === "library" ? `Library ${p.name}` : PLACES[p.kind];
    if (p.total === null) return h("li", {}, h("span", { class: "fact-label" }, name), h("span", { class: "error" }, `Couldn't be read: ${p.error}`));
    const used = p.total - p.free;
    const bar = h("span", { class: `disk-bar${p.nearly_full ? " full" : ""}`, "aria-hidden": "true" }, h("span"));
    // Set through the style object, which the page's content policy allows.
    bar.firstChild.style.inlineSize = `${Math.min(100, Math.round((used / p.total) * 100))}%`;
    return h(
      "li",
      {},
      h("span", { class: "fact-label" }, name),
      h("span", { class: "fact-value" }, `${size(p.free)} free of ${size(p.total)}`, p.nearly_full ? [" ", stateLabel("failed", "Nearly full")] : null, bar),
    );
  });
  return group("Disk space", h("p", { class: "note" }, "Measured on the server; nothing is asked outside it."), h("ul", { class: "glance disks" }, rows));
}

function fact(label, value, href, more) {
  return h("li", {}, h("span", { class: "fact-label" }, label), h("span", { class: "fact-value" }, value), href ? h("a", { href }, more) : null);
}

function tasksLine(tasks) {
  if (tasks.err) return couldNot("the tasks", tasks.err);
  const running = tasks.value.filter((t) => t.running).length;
  const failing = tasks.value.filter((t) => t.result?.startsWith("failed")).length;
  return [running ? `${plural(running, "task", "tasks")} running now` : "None running now", failing ? `; ${plural(failing, "failed", "failed")} last time.` : "; none failed last time."].join("");
}

function backupsLine(backups) {
  if (backups.err) return couldNot("the backups", backups.err);
  const newest = backups.value.copies[0];
  return newest ? `${plural(backups.value.copies.length, "copy", "copies")}; the newest made ${ago(newest.made_at)}.` : "No copies yet.";
}

function librariesLine(libraries) {
  if (libraries.err) return couldNot("the libraries", libraries.err);
  const all = libraries.value;
  const lastScan = all.reduce((max, l) => Math.max(max, l.last_scan_at ?? 0), 0);
  if (!all.length) return "None set up.";
  return `${all.map((l) => `${l.name} ${plural(l.titles, "title", "titles")}`).join(", ")}; last scanned ${lastScan ? ago(lastScan) : "never"}.`;
}

function reportsLine(reports) {
  if (reports.err) return couldNot("the reports", reports.err);
  const now = Date.now() / 1000;
  const errors = reports.value.items.filter((r) => r.level === "error" && now - r.received_at < DAY).length;
  return errors ? `${plural(errors, "error report", "error reports")} from devices in the last day.` : "No error reports from devices in the last day.";
}

export async function systemStatus(ctx) {
  const [found, places, tasks, backups, libraries, users, services, reports, about] = await Promise.all(
    [
      currentHealth(true),
      api("/admin/system/disk"),
      api("/admin/system/tasks"),
      api("/admin/system/backups"),
      api("/libraries"),
      api("/users"),
      api("/settings/services"),
      api("/diagnostics?limit=50"),
      api("/health"),
    ].map(settle),
  );
  if (!ctx.current()) return [];
  const glance = h(
    "ul",
    { class: "glance" },
    fact("Tasks", tasksLine(tasks), "#/admin/system/tasks", "See tasks"),
    fact("Backups", backupsLine(backups), "#/admin/system/backups", "See backups"),
    fact("Libraries", librariesLine(libraries), "#/admin/system/libraries", "See libraries"),
    fact(
      "Users",
      users.err ? couldNot("the users", users.err) : `${plural(users.value.length, "person", "people")}, ${plural(users.value.filter((u) => u.is_admin).length, "admin", "admins")}.`,
      "#/admin/system/users",
      "See users",
    ),
    fact(
      "Outside services",
      services.err
        ? couldNot("the services", services.err)
        : `${services.value.filter((s) => s.on).length} of ${services.value.length} in use.`,
      "#/admin/system/services",
      "See services",
    ),
    fact(
      "Diagnostics",
      reportsLine(reports),
      "#/admin/system/diagnostics",
      "See reports",
    ),
    fact("Version", about.err ? couldNot("the version", about.err) : `Melete ${about.value.version}.`),
  );
  return [health(found), disk(places), group("At a glance", glance)];
}
