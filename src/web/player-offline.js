// Offline copies from the player: pick a size, see about how big the copy will be, and download
// it once the server has made it. Shown only once an admin has turned offline copies on.

import { SignedOut, announce, api, h, size } from "/web/lib.js";

// How often a copy being made is asked about.
const FOLLOW_EVERY_MS = 3000;

// The panel for file `fileId`; `ctx.current()` says whether the page is still shown.
export function offlinePanel(fileId, ctx) {
  const choice = h("select", { id: "offline-size" });
  const status = h("p", { class: "status", role: "status" });
  const make = h("button", { type: "submit" }, "Make a copy");
  const download = h("p", { class: "offline-download" });
  const form = h("form", { class: "toolbar" }, h("div", {}, h("label", { for: "offline-size" }, "Size"), choice), make);
  const panel = h(
    "details",
    { class: "offline-copy" },
    h("summary", {}, "Download for offline"),
    h("p", { class: "muted" }, "The server makes a copy to keep on this device, then deletes it 7 days after it's downloaded."),
    form,
    status,
    download,
  );
  const c = { fileId, ctx, choice, status, make, download };
  panel.addEventListener("toggle", () => panel.open && load(c), { once: true });
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    make.disabled = true;
    try {
      const copy = await api(`/files/${fileId}/offline`, { method: "POST", body: { max_height: Number(choice.value) } });
      follow(c, copy);
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      status.textContent = err.message;
      make.disabled = false;
    }
  });
  return panel;
}

// The sizes, with estimates, and a copy already asked for, if any.
async function load(c) {
  try {
    const answer = await api(`/files/${c.fileId}/offline`);
    c.choice.replaceChildren(...answer.choices.map((o) => h("option", { value: o.max_height }, `${o.max_height}p, about ${size(o.estimated_bytes)}`)));
    const latest = answer.copies.find((copy) => copy.status !== "failed");
    if (latest) follow(c, latest);
  } catch (err) {
    if (err instanceof SignedOut) return c.ctx.signedOut();
    c.status.textContent = err.message;
  }
}

// Says how a copy is getting on until it's ready, then offers the download.
async function follow(c, copy) {
  c.make.disabled = copy.status !== "failed";
  if (copy.status === "queued") c.status.textContent = `Waiting to be made (about ${size(copy.estimated_bytes)}).`;
  if (copy.status === "making") c.status.textContent = `Making the copy: ${Math.round(copy.progress * 100)}%.`;
  if (copy.status === "failed") c.status.textContent = `The copy couldn't be made: ${copy.error ?? "no reason given"}.`;
  if (copy.status === "ready") return offer(c, copy);
  if (copy.status === "failed" || !c.ctx.current()) return;
  setTimeout(async () => {
    if (!c.ctx.current()) return;
    try {
      follow(c, await api(`/offline/${copy.id}`));
    } catch (err) {
      if (err instanceof SignedOut) return c.ctx.signedOut();
      c.status.textContent = err.message;
    }
  }, FOLLOW_EVERY_MS);
}

// A ready copy: a link to download it, and a way to delete it.
async function offer(c, copy) {
  try {
    const link = await api(`/offline/${copy.id}/link`, { method: "POST" });
    c.status.textContent = `Ready: ${size(copy.bytes)}.`;
    announce("The offline copy is ready");
    const remove = h("button", { type: "button", class: "quiet" }, "Delete the copy");
    remove.addEventListener("click", async () => {
      remove.disabled = true;
      await api(`/offline/${copy.id}`, { method: "DELETE" }).catch(() => {});
      c.download.replaceChildren();
      c.status.textContent = "Deleted from the server.";
      c.make.disabled = false;
    });
    c.download.replaceChildren(h("a", { class: "button", href: link.download_url, download: "" }, "Download"), " ", remove);
  } catch (err) {
    if (err instanceof SignedOut) return c.ctx.signedOut();
    c.status.textContent = err.message;
  }
}
