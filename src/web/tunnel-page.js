// Manage → System → Tunnel: every peer on Melete's own WireGuard tunnel now, household
// devices and watch party guests, with whether each is connected, its data, its connections,
// what it's doing and, while it's connected, where it connects from; and Remove, which cuts a
// peer off at once. It asks again every few seconds while the page is in view, and each row is
// updated in place, so a question asked in one isn't lost.

import { confirmInline, group } from "/web/forms.js";
import { SignedOut, announce, api, append, h, plural, size } from "/web/lib.js";
import { emptyState, stateLabel } from "/web/page-parts.js";
import { ago } from "/web/manage-parts.js";

const EVERY_MS = 4000;
const DOING = {
  watching: "Watching",
  listening: "Listening",
  in_party: "In a watch party",
  downloading: "Downloading",
  browsing: "Browsing",
};
const PARTS = { video: "Video", music: "Music", requests: "Requests", admin: "Admin" };
const COLUMNS = ["Who", "Kind", "Connection", "Last handshake", "Data in / out", "Connections", "Doing", "From", ""];

const secondsAgo = (s) => (s === null ? "never" : ago(Date.now() / 1000 - s));
const owner = (p) => p.user.username ?? "a removed user";

// Whose the peer is, in words: the device and its person, or the guest and their party.
function who(p) {
  if (p.kind === "device") {
    const name = p.device.name ?? "A removed device";
    return [h("strong", {}, name), h("span", { class: "note" }, ` ${owner(p)}'s; ${p.device.parts.map((x) => PARTS[x] ?? x).join(", ")}`)];
  }
  const guest = p.guest.name ? `Guest ${p.guest.name}` : "Unused guest invite";
  return [h("strong", {}, guest), h("span", { class: "note" }, ` watch party ${p.guest.party_id}, invited by ${owner(p)}`)];
}

// What it's called in the question before it's removed.
function named(p) {
  if (p.kind === "device") return `“${p.device.name ?? "this device"}” (${owner(p)}'s)`;
  return p.guest.name ? `guest ${p.guest.name}'s tunnel` : "this unused guest invite's tunnel";
}

function connection(p) {
  if (p.connected) return stateLabel("have", "Connected now");
  return p.last_handshake === null ? stateLabel("notyet", "Never connected") : stateLabel("off", "Not connected");
}

function doing(p) {
  if (!p.activity) return "Nothing asked yet";
  return [DOING[p.activity] ?? p.activity, h("span", { class: "note" }, ` ${ago(p.last_request_at)}`)];
}

// The cells after Who that change from one answer to the next.
function cells(p) {
  return [
    p.kind === "device" ? "Device" : "Guest",
    connection(p),
    secondsAgo(p.last_handshake),
    `${size(p.rx_bytes)} in / ${size(p.tx_bytes)} out`,
    String(p.connections),
    doing(p),
    p.endpoint ?? h("span", { class: "note" }, "Shown while connected"),
  ];
}

// A peer's row: Who, its cells, and Remove, whose question isn't touched by updates.
function makeRow(p, removed) {
  const head = h("th", { scope: "row" });
  const tds = COLUMNS.slice(1, -1).map((c) => h("td", { "data-label": c, class: c === "Connections" ? "r" : null }));
  const actions = h("div", { class: "row-actions" });
  const remove = h("button", { type: "button", class: "quiet" }, "Remove");
  const row = { p, el: h("tr", {}, head, tds, h("td", {}, actions)), remove };
  remove.addEventListener("click", () =>
    confirmInline(actions, {
      question: `Remove ${named(row.p)}? It's cut off from Melete at once.${row.p.kind === "device" ? " The device is deleted, so its config stops working." : ""}`,
      confirm: "Remove",
      run: async () => {
        await api(`/tunnel/peers/${row.p.address}`, { method: "DELETE" });
        announce(`Removed ${named(row.p)}`);
        await removed(row);
      },
    }),
  );
  actions.append(remove);
  return row;
}

function update(row, p) {
  row.p = p;
  const [head, ...tds] = row.el.children;
  head.replaceChildren(...who(p));
  cells(p).forEach((content, i) => {
    tds[i].replaceChildren();
    append(tds[i], [content]);
  });
  row.remove.setAttribute("aria-label", `Remove ${named(p)}`);
}

export async function tunnelPage(ctx) {
  const summary = h("p", { tabindex: "-1" });
  const problem = h("p", { class: "error", role: "alert", hidden: true });
  const body = h("tbody");
  const table = h(
    "div",
    { class: "table-wrap stacks" },
    h("table", { class: "data" }, h("caption", { class: "visually-hidden" }, "Peers on the tunnel"), h("thead", {}, h("tr", {}, COLUMNS.map((c) => h("th", { scope: "col" }, c || h("span", { class: "visually-hidden" }, "Actions"))))), body),
  );
  const empty = emptyState("No one is on the tunnel now. Household devices added under My devices on the account page, and watch party guests invited through the tunnel, show up here.", null);
  const off = h("p", { class: "status" }, "Melete's own tunnel is off on this server. Set MELETE_TUNNEL_PORT to turn it on; the remote access guide says how.");
  const rows = new Map();

  const draw = (answer) => {
    const peers = answer.peers;
    const connected = peers.filter((p) => p.connected).length;
    summary.textContent = `${plural(peers.length, "peer", "peers")} of ${answer.max_peers}, ${connected} connected now.`;
    for (const [address, row] of rows) {
      if (!peers.some((p) => p.address === address)) {
        row.el.remove();
        rows.delete(address);
      }
    }
    peers.forEach((p, i) => {
      let row = rows.get(p.address);
      if (!row) {
        row = makeRow(p, removed);
        rows.set(p.address, row);
      }
      update(row, p);
      // Moved only when out of place, so a row being asked about keeps its focus.
      const at = body.children[i] ?? null;
      if (at !== row.el) body.insertBefore(row.el, at);
    });
    table.hidden = peers.length === 0;
    empty.hidden = peers.length > 0;
  };
  // Asks again; says why when it can't, and keeps what it last showed.
  const refresh = async () => {
    try {
      draw(await api("/tunnel/peers"));
      problem.hidden = true;
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      problem.textContent = `Couldn't update the list: ${err.message}`;
      problem.hidden = false;
    }
  };
  const removed = async (row) => {
    const next = row.el.nextElementSibling ?? row.el.previousElementSibling;
    await refresh();
    (next?.querySelector("button") ?? summary).focus();
  };

  let first;
  try {
    first = await api("/tunnel/peers");
  } catch (err) {
    if (err.status === 409) return [group("Peers", off)];
    throw err;
  }
  if (!ctx.current()) return [];
  draw(first);
  follow(ctx, refresh);
  return [
    group(
      "Peers",
      h("p", { class: "note" }, "Everyone on Melete's own WireGuard tunnel now. Where a peer connects from is shown only while it's connected, and isn't kept. Updates every few seconds while this page is open."),
      summary,
      problem,
      empty,
      table,
    ),
  ];
}

// Calls `refresh` every few seconds while the page is open and in view.
function follow(ctx, refresh) {
  let timer = null;
  const next = () => {
    clearTimeout(timer);
    timer = null;
    if (!ctx.current()) return document.removeEventListener("visibilitychange", seen);
    if (!document.hidden) timer = setTimeout(() => refresh().finally(next), EVERY_MS);
  };
  const seen = () => {
    if (!ctx.current()) return next();
    if (!document.hidden && timer === null) refresh().finally(next);
  };
  document.addEventListener("visibilitychange", seen);
  next();
}
