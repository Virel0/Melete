// Share, on the owner's own playlist: who it's shared with and how (can edit, or listen only),
// sharing it with someone else from the household's users, changing how, and stopping a share
// after asking (`/aoide/shares`, `GET /users/directory`). Those it's shared with see it in
// their Aoide apps and here; those who can edit change it on their Aoide devices, through the
// same sync log, so everyone's copy follows.

import { confirmInline } from "/web/forms.js";
import { SignedOut, api, h } from "/web/lib.js";
import { hold, placeholders, release } from "/web/page-parts.js";

let made = 0;

const how = (share) => (share.canEdit ? "can edit" : "can listen");

// What went wrong with a share, in a sentence.
function shareTrouble(err) {
  if (err.status === 404) return "This playlist hasn't reached the server's sync log yet. Try again in a moment.";
  if (err.status === 403) return "Only the playlist's owner shares it.";
  return err.message;
}

// The section, drawn into `box` and again after each change; `say` reports what happened.
export function shareSection(p) {
  const id = `share-${++made}`;
  const said = h("p", { class: "note", role: "status" });
  const body = h("div", {}, placeholders("who it's shared with", { kind: "lines", count: 2 }));
  const box = h(
    "section",
    { class: "playlist-share", "aria-labelledby": `${id}-title` },
    h("h2", { id: `${id}-title` }, "Share"),
    h("p", { class: "note" }, "People you share it with see it here and in their Aoide apps. Those who can edit change it on their Aoide devices, for everyone it's shared with."),
    body,
    said,
  );

  const send = async (button, request, done) => {
    if (!hold(button)) return;
    said.textContent = "";
    try {
      await api(...request);
      await draw(done);
    } catch (err) {
      if (err instanceof SignedOut) throw err;
      said.textContent = `Couldn't change it: ${shareTrouble(err)}`;
      release(button);
    }
  };
  const share = (button, grantee, canEdit, done) =>
    send(button, ["/aoide/shares", { method: "POST", body: { playlistId: p.id, granteeUserId: grantee, canEdit } }], done);

  const row = (s) => {
    const actions = h("div", { class: "row-actions" });
    const change = h("button", { type: "button", class: "quiet small" }, s.canEdit ? "Make listen only" : "Let them edit");
    change.addEventListener("click", () =>
      share(change, s.granteeUserId, !s.canEdit, s.canEdit ? `${s.granteeName} can listen to it now, not change it.` : `${s.granteeName} can edit it now.`),
    );
    const stop = h("button", { type: "button", class: "quiet small" }, "Stop sharing");
    stop.addEventListener("click", () =>
      confirmInline(actions, {
        question: `Stop sharing with ${s.granteeName}? It goes from their apps and changes stop reaching them; nothing they already changed is undone.`,
        confirm: "Stop sharing",
        keep: "Keep sharing",
        run: () => send(stop, [`/aoide/shares/${encodeURIComponent(p.id)}/${encodeURIComponent(s.granteeUserId)}`, { method: "DELETE" }], `Stopped sharing with ${s.granteeName}.`),
      }),
    );
    actions.append(change, stop);
    return h("li", {}, h("div", {}, h("strong", {}, s.granteeName), ` · ${how(s)}`), actions);
  };

  const addForm = (people) => {
    const pick = h("select", { id: `${id}-who` }, people.map((u) => h("option", { value: String(u.id) }, u.username)));
    const edit = h("input", { type: "checkbox", id: `${id}-edit`, checked: true });
    const button = h("button", { type: "submit" }, "Share");
    const form = h(
      "form",
      { class: "share-add" },
      h("div", { class: "field" }, h("label", { for: `${id}-who` }, "Share with"), pick),
      h("div", { class: "check" }, edit, h("label", { for: `${id}-edit` }, "They can edit it")),
      h("div", { class: "form-actions" }, button),
    );
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      const who = people.find((u) => String(u.id) === pick.value);
      share(button, who.id, edit.checked, `Shared with ${who.username}; they ${edit.checked ? "can edit it" : "can listen to it"}.`);
    });
    return form;
  };

  async function draw(done) {
    const [all, users] = await Promise.all([api("/aoide/shares"), api("/users/directory")]);
    const mine = all.filter((s) => s.playlistId === p.id && s.isOwner);
    const shared = new Set(mine.map((s) => s.granteeUserId));
    const people = users.filter((u) => u.username !== p.owner && !shared.has(String(u.id)));
    body.replaceChildren(
      mine.length ? h("ul", { class: "rows share-list", "aria-label": "Shared with" }, mine.map(row)) : h("p", {}, "Not shared with anyone yet."),
      people.length ? addForm(people) : h("p", { class: "note" }, "It's shared with everyone else on this server."),
    );
    if (done) {
      said.textContent = done;
      body.querySelector("button")?.focus();
    }
  }
  draw().catch((err) => {
    if (err instanceof SignedOut) throw err;
    body.replaceChildren(h("p", { class: "error" }, `Couldn't load who it's shared with: ${shareTrouble(err)}`));
  });
  return box;
}
