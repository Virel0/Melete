// Collections: saved song filters, each the listener's own (`/music/collections`), over several
// genres, a span of years and "only songs I like". The Songs page narrows its list with them
// (collectionFilter) and saves what it shows as one; Listen's sidebar lists them, each opening
// the Songs page so narrowed (collectionHref).
//
// collectionFilter(genres, params, change) gives the panel for the Songs page: `genres` are the
// library's, `params` the address's, and `change(key, value)` sets one of KEYS in the list.

import { SignedOut, api, h } from "/web/lib.js";
import { hold, release } from "/web/page-parts.js";

// The list's query parameters a collection sets.
export const KEYS = ["genres", "year_from", "year_to", "liked"];

// The Songs page narrowed to a collection.
export function collectionHref(c) {
  const params = new URLSearchParams();
  if (c.genres.length) params.set("genres", c.genres.join(";"));
  if (c.year_from) params.set("year_from", c.year_from);
  if (c.year_to) params.set("year_to", c.year_to);
  if (c.liked) params.set("liked", "true");
  return `#/music/songs?${params}`;
}

// A collection changed here: the sidebar asks again.
const changed = () => document.dispatchEvent(new CustomEvent("melete:collections"));

function genreBoxes(genres, chosen, onChange) {
  const picked = new Set(chosen.map((g) => g.toLowerCase()));
  const boxes = genres.map((g, i) => {
    const box = h("input", { type: "checkbox", id: `collection-genre-${i}`, value: g.name, checked: picked.has(g.name.toLowerCase()) });
    box.addEventListener("change", onChange);
    return h("li", { class: "check" }, box, h("label", { for: `collection-genre-${i}` }, g.name));
  });
  return h("fieldset", { class: "collection-genres" }, h("legend", {}, "Genres"), h("ul", {}, boxes));
}

function yearBox(id, label, value) {
  return h("div", {}, h("label", { for: id }, label), h("input", { id, type: "number", inputmode: "numeric", min: "1", max: "9999", value: value ?? "" }));
}

// Save as a collection: a name, then Save; what happened said under it.
function saveForm(values, saved) {
  const name = h("input", { id: "collection-name", type: "text", maxlength: "100", autocomplete: "off", required: true });
  const said = h("p", { class: "note", role: "status" });
  const button = h("button", { type: "submit", class: "quiet" }, "Save as a collection");
  const form = h("form", { class: "collection-save" }, h("div", {}, h("label", { for: "collection-name" }, "Collection name"), name), button, said);
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const v = values();
    if (!v.genres.length && !v.year_from && !v.year_to && !v.liked) {
      said.textContent = "Choose a genre, a year or liked songs first.";
      return;
    }
    if (!name.value.trim()) {
      said.textContent = "Give it a name first.";
      name.focus();
      return;
    }
    if (!hold(button)) return;
    said.textContent = "";
    try {
      const c = await api("/music/collections", { method: "POST", body: { name: name.value.trim(), ...v } });
      said.textContent = `Saved ${c.name}. It's in the sidebar under Collections.`;
      name.value = "";
      changed();
      saved();
    } catch (err) {
      if (err instanceof SignedOut) throw err;
      said.textContent = `Couldn't save it: ${err.message}`;
    } finally {
      release(button);
    }
  });
  return form;
}

// The listener's collections, each opening the list narrowed to it, with Delete.
function savedList() {
  const list = h("ul", { class: "rows collection-list" });
  const said = h("p", { class: "note", role: "status" });
  const draw = async () => {
    const answer = await api("/music/collections").catch((err) => {
      if (err instanceof SignedOut) throw err;
      return { items: [] };
    });
    list.replaceChildren(
      ...answer.items.map((c) => {
        const remove = h("button", { type: "button", class: "quiet small" }, "Delete", h("span", { class: "visually-hidden" }, ` ${c.name}`));
        remove.addEventListener("click", async () => {
          if (!hold(remove)) return;
          try {
            await api(`/music/collections/${c.id}`, { method: "DELETE" });
            said.textContent = `Deleted ${c.name}.`;
            changed();
            await draw();
            list.closest("details")?.querySelector("summary")?.focus();
          } catch (err) {
            if (err instanceof SignedOut) throw err;
            said.textContent = `Couldn't delete it: ${err.message}`;
            release(remove);
          }
        });
        return h("li", {}, h("a", { href: collectionHref(c) }, c.name), remove);
      }),
    );
    section.hidden = !answer.items.length;
  };
  const section = h("section", { class: "collection-saved", hidden: true }, h("h3", {}, "Your collections"), list, said);
  return { section, draw };
}

export function collectionFilter(genres, params, change) {
  const chosen = (params.get("genres") ?? "").split(";").filter(Boolean);
  const onGenres = () => change("genres", [...fieldset.querySelectorAll("input:checked")].map((b) => b.value).join(";"));
  const fieldset = genres.length ? genreBoxes(genres, chosen, onGenres) : h("fieldset", { hidden: true });
  const from = yearBox("year-from", "From year", params.get("year_from"));
  const to = yearBox("year-to", "To year", params.get("year_to"));
  for (const [box, key] of [[from, "year_from"], [to, "year_to"]]) {
    const input = box.querySelector("input");
    input.addEventListener("change", () => change(key, /^\d{1,4}$/.test(input.value) ? input.value : ""));
  }
  const liked = h("input", { type: "checkbox", id: "collection-liked", checked: params.get("liked") === "true" });
  liked.addEventListener("change", () => change("liked", liked.checked ? "true" : ""));
  const values = () => {
    const year = (box) => (/^\d{1,4}$/.test(box.querySelector("input").value) ? Number(box.querySelector("input").value) : null);
    return { genres: [...fieldset.querySelectorAll("input:checked")].map((b) => b.value), year_from: year(from), year_to: year(to), liked: liked.checked };
  };
  const saved = savedList();
  saved.draw().catch(() => {});
  const open = chosen.length || KEYS.slice(1).some((k) => params.get(k));
  const node = h(
    "details",
    { class: "collection-filter", open: Boolean(open) },
    h("summary", {}, "Genres, years and likes"),
    fieldset,
    h("div", { class: "collection-years" }, from, to),
    h("div", { class: "check" }, liked, h("label", { for: "collection-liked" }, "Only songs I like")),
    saveForm(values, () => saved.draw().catch(() => {})),
    saved.section,
  );
  // Clearing the list's filters clears these boxes too.
  const clear = () => {
    for (const box of node.querySelectorAll("input[type=checkbox]")) box.checked = false;
    for (const box of node.querySelectorAll("input[type=number]")) box.value = "";
  };
  return { node, clear };
}
