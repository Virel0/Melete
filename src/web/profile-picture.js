// Your profile picture, on Your account: the one you have, uploading one (read and typed in
// the browser first, as playlist pictures are), choosing one from the gallery (Melete's own
// pictures, then the server's gallery folder, a category at a time), and removing it. Every
// app shows it beside your name.

import { group } from "/web/forms.js";
import { SignedOut, announce, api, h } from "/web/lib.js";
import { PICTURE_TYPES, readPicture } from "/web/picture-file.js";

const PAGE = 60;

// The picture, or the name's first letter in a circle without one.
export function avatar(user, size = "large") {
  const cls = `avatar avatar-${size}`;
  return user.avatar_url
    ? h("img", { class: cls, src: user.avatar_url, alt: "" })
    : h("span", { class: `${cls} avatar-none`, "aria-hidden": "true" }, (user.username || "?").slice(0, 1).toUpperCase());
}

export async function pictureGroup() {
  const [me, gallery] = await Promise.all([api("/me"), api("/avatars")]);
  const said = h("p", { class: "form-result", role: "status" });
  const shown = h("div", { class: "avatar-current" });
  const remove = h("button", { type: "button", class: "quiet" }, "Remove picture");
  const draw = (user) => {
    shown.replaceChildren(avatar(user));
    remove.hidden = !user.avatar_url;
    // The header's, beside the name.
    document.querySelector(".account summary .avatar")?.replaceWith(avatar(user, "small"));
  };
  draw(me);

  const change = async (path, request, words) => {
    said.textContent = "Saving…";
    try {
      const answer = await api(path, request);
      draw({ ...me, avatar_url: answer.avatar_url });
      said.textContent = words;
      announce(words);
    } catch (err) {
      if (err instanceof SignedOut) throw err;
      said.textContent = `Couldn't change your picture: ${err.message}`;
    }
  };

  const input = h("input", { type: "file", id: "avatar-upload", accept: PICTURE_TYPES.join(","), "aria-describedby": "avatar-upload-hint" });
  input.addEventListener("change", async () => {
    const chosen = input.files?.[0];
    if (!chosen) return;
    const read = await readPicture(chosen);
    input.value = "";
    if (read.error) {
      said.textContent = read.error;
      return;
    }
    await change("/me/avatar", { method: "PUT", body: read.blob }, "Picture saved.");
  });
  remove.addEventListener("click", () => change("/me/avatar", { method: "DELETE" }, "Picture removed."));

  const choose = gallery.categories.length ? galleryPicker(gallery.categories, (id) => change("/me/avatar/gallery", { method: "PUT", body: { id } }, "Picture saved.")) : null;
  return group(
    "Profile picture",
    h("p", { class: "note" }, "Shown beside your name in Melete and in the apps, to everyone in the household."),
    shown,
    h(
      "div",
      { class: "field" },
      h("label", { for: "avatar-upload" }, "Upload a picture"),
      h("p", { class: "note", id: "avatar-upload-hint" }, "JPEG, PNG or WebP, up to 5 MB."),
      input,
    ),
    choose,
    h("div", { class: "form-actions" }, remove),
    said,
  );
}

// Choose from the gallery: a category, then its pictures a page at a time, each a button.
function galleryPicker(categories, pick) {
  const select = h(
    "select",
    { id: "avatar-category" },
    categories.map((c) => h("option", { value: c.id }, `${c.name} (${c.count})`)),
  );
  const grid = h("ul", { class: "avatar-grid", "aria-label": "Pictures" });
  const more = h("button", { type: "button", class: "quiet" }, "Show more");
  const status = h("p", { class: "note", role: "status" });
  let offset = 0;
  const load = async (fresh) => {
    if (fresh) {
      offset = 0;
      grid.replaceChildren();
    }
    const category = select.value;
    const name = categories.find((c) => c.id === category)?.name ?? "";
    status.textContent = "Loading pictures…";
    try {
      const page = await api(`/avatars/pictures?${new URLSearchParams({ category, offset, limit: PAGE })}`);
      if (select.value !== category) return;
      page.pictures.forEach((p, i) => {
        const n = offset + i + 1;
        const button = h("button", { type: "button", class: "avatar-choice", "aria-label": `Use picture ${n} of ${name}` }, h("img", { src: p.url, alt: "", loading: "lazy" }));
        button.addEventListener("click", () => pick(p.id));
        grid.append(h("li", {}, button));
      });
      offset += page.pictures.length;
      more.hidden = offset >= page.total;
      status.textContent = `${offset} of ${page.total} shown.`;
    } catch (err) {
      if (err instanceof SignedOut) throw err;
      status.textContent = `Couldn't load the pictures: ${err.message}`;
    }
  };
  const open = h("button", { type: "button", class: "quiet", "aria-expanded": "false" }, "Choose from the gallery");
  const panel = h("div", { class: "avatar-gallery", hidden: true }, h("div", { class: "field" }, h("label", { for: "avatar-category" }, "Category"), select), grid, h("div", { class: "form-actions" }, more), status);
  open.addEventListener("click", () => {
    const opening = panel.hidden;
    panel.hidden = !opening;
    open.setAttribute("aria-expanded", String(opening));
    if (opening && !grid.children.length) load(true);
  });
  select.addEventListener("change", () => load(true));
  more.addEventListener("click", () => load(false));
  return h("div", {}, h("div", { class: "form-actions" }, open), panel);
}
