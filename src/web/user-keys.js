// A user's API keys, shown to an admin under their row in Users: each key's name, what it may
// reach, and when it was made and last used. Never the key itself; their sessions stay theirs.
// An admin revokes a key here, after a question naming it; its owner can under Your account, and
// a new password signs them out everywhere.

import { confirmInline, when } from "/web/forms.js";
import { SignedOut, announce, api, h, plural } from "/web/lib.js";
import { SCOPES } from "/web/account.js";

// The button that opens and closes the list, and the box it opens, to go under the row.
export function userKeys(page, u) {
  const id = `keys-${u.id}`;
  const box = h("div", { id, class: "user-keys", hidden: true });
  const button = h("button", { type: "button", class: "quiet", "aria-expanded": "false", "aria-controls": id }, "API keys");
  // Asks for the keys afresh and draws them; after a revoke, focus goes back to the list.
  const load = async (refocus) => {
    try {
      box.replaceChildren(...describe(u, await api(`/users/${u.id}/keys`), load));
    } catch (err) {
      if (err instanceof SignedOut) return page.ctx.signedOut();
      box.replaceChildren(h("p", { class: "error", role: "alert", tabindex: "-1" }, `Couldn't load their keys. ${err.message}`));
    }
    if (refocus) (box.querySelector("li button") ?? box.firstElementChild)?.focus();
  };
  button.addEventListener("click", async () => {
    const open = button.getAttribute("aria-expanded") === "true";
    button.setAttribute("aria-expanded", String(!open));
    box.hidden = open;
    if (open) return;
    box.replaceChildren(h("p", { class: "note", role: "status" }, "Loading their keys…"));
    await load(false);
  });
  return { button, box };
}

function describe(u, keys, load) {
  if (!keys.length) return [h("p", { class: "note", tabindex: "-1" }, `${u.username} has no API keys.`)];
  return [
    h("h3", { class: "edit-title", tabindex: "-1" }, `${u.username}'s API keys: ${plural(keys.length, "key", "keys")}`),
    h(
      "ul",
      { class: "rows" },
      keys.map((k) => keyRow(u, k, load)),
    ),
    h("p", { class: "note" }, "They can revoke a key under Your account too; a new password signs out every key and session they have."),
  ];
}

// One key, with Revoke, which asks first and says what it does to the apps using it.
function keyRow(u, k, load) {
  const actions = h("div", { class: "row-actions" });
  const revoke = h("button", { type: "button", class: "quiet", "aria-label": `Revoke ${k.name}` }, "Revoke");
  revoke.addEventListener("click", () =>
    confirmInline(actions, {
      question: `Revoke ${u.username}'s key “${k.name}”? Apps using it are signed out at once.`,
      confirm: "Revoke key",
      run: async () => {
        await api(`/users/${u.id}/keys/${k.id}`, { method: "DELETE" });
        announce(`Revoked ${u.username}'s key ${k.name}`);
        await load(true);
      },
    }),
  );
  actions.append(revoke);
  return h(
    "li",
    {},
    h(
      "div",
      {},
      h("strong", {}, k.name),
      h("p", { class: "note" }, `${(SCOPES[k.scope] ?? k.scope).replace("you", "they")}. Made ${when(k.created_at)}; last used ${when(k.last_used_at)}.`),
    ),
    actions,
  );
}
