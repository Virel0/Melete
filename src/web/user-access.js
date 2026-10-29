// What a user can see, set by an admin from their row under Users: every library or some, and
// the highest content rating.

import { form } from "/web/forms.js";
import { api, h } from "/web/lib.js";

const AGES = [0, 6, 7, 9, 10, 12, 13, 14, 15, 16, 17, 18];
const KINDS = { movies: "movies", shows: "series", music: "music" };

// A row swapped for the form; Cancel puts it back, Save says what was set.
export async function accessEditor(page, u, row) {
  const before = [...row.childNodes];
  const back = () => {
    row.replaceChildren(...before);
    row.querySelector("button")?.focus();
  };
  const [now, libraries] = await Promise.all([api(`/users/${u.id}/access`), api("/libraries")]);
  const id = (part) => `access-${u.id}-${part}`;
  const every = h("input", { type: "radio", name: id("which"), id: id("every"), checked: now.libraries === null });
  const some = h("input", { type: "radio", name: id("which"), id: id("some"), checked: now.libraries !== null });
  const boxes = libraries.map((l) =>
    h("input", { type: "checkbox", id: id(`library-${l.id}`), value: l.name, checked: now.libraries?.includes(l.name) ?? false }),
  );
  const checks = libraries.map((l, i) => h("div", { class: "check" }, boxes[i], h("label", { for: boxes[i].id }, `${l.name} (${KINDS[l.kind] ?? l.kind})`)));
  const showList = () => {
    for (const box of boxes) box.disabled = every.checked;
  };
  every.addEventListener("change", showList);
  some.addEventListener("change", showList);
  showList();
  const age = h(
    "select",
    { id: id("age") },
    h("option", { value: "" }, "No limit"),
    AGES.map((a) => h("option", { value: a, selected: now.max_age === a }, a === 0 ? "Suitable for all ages" : `Up to age ${a}`)),
  );
  const which = {
    box: h(
      "div",
      {},
      h(
        "fieldset",
        { class: "choices" },
        h("legend", {}, "Libraries"),
        h("div", { class: "check" }, every, h("label", { for: every.id }, "Every library, and any added later")),
        h("div", { class: "check" }, some, h("label", { for: some.id }, "Only those ticked below")),
      ),
      h("fieldset", { class: "choices" }, h("legend", {}, "Libraries they can see"), checks),
    ),
    clear() {},
  };
  const rating = {
    box: h(
      "div",
      { class: "field" },
      h("label", { for: age.id }, "Highest rating"),
      h(
        "p",
        { class: "note", id: id("age-hint") },
        "Movies and series rated for older viewers are hidden, and so is any without a rating Melete can read. Music has no ratings: only its libraries count. Someone who sees only some music libraries can't use Aoide's sync, playlists or listening together.",
      ),
      age,
    ),
    clear() {},
  };
  age.setAttribute("aria-describedby", id("age-hint"));
  const editor = form([which, rating], "Save", async () => {
    const libs = every.checked ? null : boxes.filter((b) => b.checked).map((b) => b.value);
    const body = { libraries: libs, max_age: age.value === "" ? null : Number(age.value) };
    const saved = await api(`/users/${u.id}/access`, { method: "PUT", body });
    back();
    page.tell(describe(u.username, saved));
    return null;
  });
  editor.querySelector(".form-actions").append(h("button", { type: "button", class: "quiet", onclick: back }, "Cancel"));
  editor.addEventListener("keydown", (event) => {
    if (event.key === "Escape") back();
  });
  row.replaceChildren(h("div", {}, h("h3", { class: "edit-title" }, `What ${u.username} can see`), editor));
  (now.libraries === null ? every : some).focus();
}

// What was set, in words.
function describe(name, saved) {
  const where = saved.libraries === null ? "every library" : saved.libraries.length ? saved.libraries.join(", ") : "no library";
  const rated = saved.max_age === null ? "" : `, rated up to age ${saved.max_age}`;
  return `${name} can see ${where}${rated}.`;
}
