// Settings → Diagnostics, for admins: the reports apps send about themselves,
// newest first, a page at a time, each with its details on request.

import { confirmInline, group, when } from "/web/forms.js";
import { SignedOut, api, h, number, plural } from "/web/lib.js";

const PAGE = 50;
const LEVELS = { info: "Info", warning: "Warning", error: "Error" };

export async function diagnostics(ctx) {
  const first = await api(`/diagnostics?limit=${PAGE}`);
  if (!ctx.current()) return [];
  const intro = h(
    "p",
    { class: "note" },
    "Apps send these about themselves. Each is kept 30 days, and at most 100 from one device. Anything that looked like a password or key was replaced before it was kept.",
  );
  if (!first.total) return [group("Reports from apps", intro, h("p", { class: "status" }, "No reports in the last 30 days."))];

  const list = h("ul", { class: "rows" });
  const more = h("div", { class: "more" });
  let shown = 0;

  const show = (page) => {
    const rows = page.items.map((report) => reportRow(ctx, report));
    list.append(...rows);
    shown += page.items.length;
    const left = page.total - shown;
    if (left <= 0) {
      more.replaceChildren();
      return rows;
    }
    const button = h("button", { type: "button", class: "quiet" }, `Show more (${number.format(left)} left)`);
    button.addEventListener("click", async () => {
      button.disabled = true;
      try {
        const next = await api(`/diagnostics?limit=${PAGE}&offset=${shown}`);
        show(next)[0]?.querySelector("button")?.focus();
      } catch (err) {
        if (err instanceof SignedOut) return ctx.signedOut();
        button.disabled = false;
        more.append(h("p", { class: "error" }, err.message));
      }
    });
    more.replaceChildren(button);
    return rows;
  };
  show(first);
  return [group("Reports from apps", intro, h("p", { class: "note" }, `${plural(first.total, "report", "reports")}, newest first.`), list, more)];
}

// One report: what it says, which app and device sent it, its details on request, and Delete
// and Delete all from this device.
function reportRow(ctx, report) {
  const { details, toggle } = detailsToggle(ctx, report);
  const app = [report.app, report.app_version].filter(Boolean).join(" ");
  const actions = h("div", { class: "row-actions" }, toggle);
  const remove = h("button", { type: "button", class: "quiet" }, "Delete");
  remove.addEventListener("click", () =>
    confirmInline(actions, {
      question: `Delete “${report.summary}” from ${report.device}?`,
      confirm: "Delete report",
      keep: "Keep it",
      run: async () => {
        await api(`/diagnostics/${report.id}`, { method: "DELETE" });
        ctx.reload(`Deleted “${report.summary}”.`);
      },
    }),
  );
  const device = h("button", { type: "button", class: "quiet" }, "Delete all from this device");
  device.addEventListener("click", () =>
    confirmInline(actions, {
      question: `Delete every report from ${report.device} (${report.user})? New ones it sends are kept as before.`,
      confirm: `Delete ${report.device}'s reports`,
      keep: "Keep them",
      run: async () => {
        const done = await api(`/diagnostics?${new URLSearchParams({ user: report.user, device: report.device })}`, { method: "DELETE" });
        ctx.reload(`Deleted ${plural(done.deleted, "report", "reports")} from ${report.device}.`);
      },
    }),
  );
  actions.append(remove, device);
  return h(
    "li",
    {},
    h(
      "div",
      {},
      h("strong", {}, report.summary),
      " ",
      h("span", { class: "badge" }, LEVELS[report.level] ?? report.level),
      h(
        "p",
        { class: "note" },
        `${report.platform ? `${app} on ${report.platform}` : app}. From ${report.device} (${report.user}), ${when(report.received_at)}.`,
      ),
      details,
    ),
    actions,
  );
}

// Show details and Hide details, loading a report's details the first time.
function detailsToggle(ctx, report) {
  const detailsId = `report-${report.id}`;
  const details = h("pre", { id: detailsId, class: "report-details", hidden: true, tabindex: "-1" });
  const toggle = h("button", { type: "button", class: "quiet", "aria-expanded": "false", "aria-controls": detailsId }, "Show details");
  toggle.addEventListener("click", async () => {
    const open = toggle.getAttribute("aria-expanded") === "true";
    if (open) {
      details.hidden = true;
      toggle.setAttribute("aria-expanded", "false");
      toggle.textContent = "Show details";
      return;
    }
    if (!details.textContent) {
      toggle.disabled = true;
      try {
        const full = await api(`/diagnostics/${report.id}`);
        details.textContent = JSON.stringify(full.details, null, 2);
      } catch (err) {
        if (err instanceof SignedOut) return ctx.signedOut();
        details.textContent = `Couldn't load the details: ${err.message}`;
      } finally {
        toggle.disabled = false;
      }
    }
    details.hidden = false;
    toggle.setAttribute("aria-expanded", "true");
    toggle.textContent = "Hide details";
  });
  return { details, toggle };
}
