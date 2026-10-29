// Users: adding, renaming, making or taking admin, what each can see, their API keys, new
// passwords and deleting, for admins.

import { confirmInline, field, form, group, secret } from "/web/forms.js";
import { SignedOut, announce, api, append, h } from "/web/lib.js";
import { accessEditor } from "/web/user-access.js";
import { userKeys } from "/web/user-keys.js";

export async function users(ctx) {
  const all = await api("/users");
  if (!ctx.current()) return [];
  // What the rows share: the list, where a new password is shown, and the line that says what
  // just happened.
  const page = {
    ctx,
    list: h("ul", { class: "rows" }),
    shown: h("div"),
    said: h("p", { class: "flash", role: "status", hidden: true }),
  };
  page.refresh = async () => draw(page, await api("/users"));
  page.tell = (message) => {
    page.said.textContent = message;
    page.said.hidden = false;
  };
  draw(page, all);
  return [
    group(
      "Users",
      h("p", { class: "note" }, "Admins change settings and manage users. There's always at least one, and nobody takes away their own."),
      page.said,
      page.list,
      page.shown,
    ),
    group("Add a user", addForm(page)),
  ];
}

// Draws every user's row.
function draw(page, items) {
  page.list.replaceChildren(...items.map((u) => userRow(page, u)));
}

// One user: their name and role, with Rename, and for anyone else Make admin or Remove admin,
// What they can see (not for admins, who see everything), Reset password and Delete.
function userRow(page, u) {
  const { ctx } = page;
  const actions = h("div", { class: "row-actions" });
  const row = h("li", { "data-user": u.id });
  actions.append(h("button", { type: "button", class: "quiet", onclick: () => renaming(page, u, row) }, u.id === ctx.me.id ? "Rename yourself" : "Rename"));
  if (u.id !== ctx.me.id) adminButton(page, u, actions);
  // Admins see everything, so only others have a choice.
  if (!u.is_admin) {
    const what = h("button", { type: "button", class: "quiet" }, "What they can see");
    what.addEventListener("click", () =>
      accessEditor(page, u, row).catch((err) => {
        if (err instanceof SignedOut) return ctx.signedOut();
        page.tell(err.message);
      }),
    );
    actions.append(what);
  }
  if (u.id !== ctx.me.id) {
    actions.append(
      h(
        "button",
        {
          type: "button",
          class: "quiet",
          onclick: () =>
            confirmInline(actions, {
              question: `Give ${u.username} a new password? They're signed out everywhere.`,
              confirm: "Make new password",
              run: async () => {
                const reset = await api(`/users/${u.id}/password`, { method: "POST" });
                page.shown.replaceChildren(secret(`New password for ${reset.username}`, reset.password, "Give it to them now: it won't be shown again."));
                await page.refresh();
              },
            }),
        },
        "Reset password",
      ),
      h(
        "button",
        {
          type: "button",
          class: "quiet",
          onclick: () =>
            confirmInline(actions, {
              question: `Delete ${u.username}? Their sessions, API keys and watch progress go too. This can't be undone.`,
              confirm: `Delete ${u.username}`,
              run: async () => {
                await api(`/users/${u.id}`, { method: "DELETE" });
                announce(`Deleted ${u.username}`);
                await page.refresh();
                page.list.querySelector("button")?.focus();
              },
            }),
        },
        "Delete",
      ),
    );
  }
  // Their API keys and what each may reach, opened under the row.
  const keys = userKeys(page, u);
  actions.insertBefore(keys.button, [...actions.children].find((b) => b.textContent === "Reset password") ?? null);
  append(row, [
    h("div", {}, h("strong", {}, u.username), " ", u.is_admin ? h("span", { class: "badge" }, "Admin") : null, u.id === ctx.me.id ? h("span", { class: "note" }, " (you)") : null),
    actions,
    keys.box,
  ]);
  return row;
}

// Make admin for someone who isn't one; Remove admin, asked first, for someone who is.
function adminButton(page, u, actions) {
  const { ctx } = page;
  if (!u.is_admin) {
    const make = h("button", { type: "button", class: "quiet" }, "Make admin");
    make.addEventListener("click", async () => {
      make.disabled = true;
      try {
        await setAdmin(page, u, true);
      } catch (err) {
        if (err instanceof SignedOut) return ctx.signedOut();
        make.disabled = false;
        page.tell(err.message);
      }
    });
    actions.append(make);
  }
  if (u.is_admin) {
    actions.append(
      h(
        "button",
        {
          type: "button",
          class: "quiet",
          onclick: () =>
            confirmInline(actions, {
              question: `Take admin from ${u.username}? They keep their account but can't change settings or manage users.`,
              confirm: "Remove admin",
              run: () => setAdmin(page, u, false),
            }),
        },
        "Remove admin",
      ),
    );
  }
}

// Makes someone an admin, or not, and says so with focus back on their row.
async function setAdmin(page, u, isAdmin) {
  await api(`/users/${u.id}`, { method: "PUT", body: { username: u.username, is_admin: isAdmin } });
  await page.refresh();
  page.tell(isAdmin ? `${u.username} is an admin now.` : `${u.username} isn't an admin any more.`);
  page.list.querySelector(`[data-user="${u.id}"] button`)?.focus();
}

// A row's name swapped for a form that renames them; Cancel puts it back.
function renaming(page, u, row) {
  const { ctx } = page;
  const before = [...row.childNodes];
  const name = field(`rename-${u.id}`, `New name for ${u.username}`, h("input", { value: u.username, autocomplete: "off", spellcheck: "false", maxlength: 64 }));
  const back = () => {
    row.replaceChildren(...before);
    row.querySelector("button")?.focus();
  };
  const renameForm = form([name], "Save name", async () => {
    const wanted = name.input.value.trim();
    if (!/^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/.test(wanted)) {
      name.fail("Use 1 to 64 letters, digits, dots, dashes or underscores, starting with a letter or digit.");
      return null;
    }
    if (wanted === u.username) {
      back();
      return null;
    }
    try {
      await api(`/users/${u.id}`, { method: "PUT", body: { username: wanted, is_admin: u.is_admin } });
    } catch (err) {
      if (err.status === 409) {
        name.fail("That username is taken.");
        return null;
      }
      throw err;
    }
    if (u.id === ctx.me.id) {
      ctx.me.username = wanted;
      document.getElementById("username").textContent = wanted;
    }
    await page.refresh();
    page.tell(`Renamed ${u.username} to ${wanted}. They sign in with the new name from now on.`);
    page.list.querySelector(`[data-user="${u.id}"] button`)?.focus();
    return null;
  });
  renameForm.querySelector(".form-actions").append(h("button", { type: "button", class: "quiet", onclick: back }, "Cancel"));
  renameForm.addEventListener("keydown", (event) => {
    if (event.key === "Escape") back();
  });
  row.replaceChildren(h("div", {}, renameForm));
  name.input.focus();
  name.input.select();
}

// Adding a user: a name, a password, and whether they're an admin.
function addForm(page) {
  const username = field("new-username", "Username", h("input", { autocomplete: "off", spellcheck: "false" }), "Letters, digits, dots, dashes and underscores.");
  const password = field("new-user-password", "Password", h("input", { type: "password", autocomplete: "new-password" }), "At least 8 characters. They can change it later.");
  const admin = h("input", { type: "checkbox", id: "new-user-admin" });
  const adminBox = {
    box: h("div", { class: "check" }, admin, h("label", { for: "new-user-admin" }, "Admin: can change settings and manage users")),
    clear() {},
  };
  return form([username, password, adminBox], "Add user", async () => {
    const name = username.input.value.trim();
    if (!/^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/.test(name)) {
      username.fail("Use 1 to 64 letters, digits, dots, dashes or underscores, starting with a letter or digit.");
      return null;
    }
    if (password.input.value.length < 8) {
      password.fail("Use at least 8 characters.");
      return null;
    }
    try {
      await api("/users", { method: "POST", body: { username: name, password: password.input.value, is_admin: admin.checked } });
    } catch (err) {
      if (err.status === 409) {
      username.fail("That username is taken.");
      return null;
    }
      throw err;
    }
    username.input.value = password.input.value = "";
    admin.checked = false;
    await page.refresh();
    return `Added ${name}.`;
  });
}
