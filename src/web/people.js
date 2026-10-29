// Who's in a title and who made it, a person's titles in the library, and a franchise's films.

import { api, h, loading, metaRow, plural, show } from "/web/lib.js";
import { art, titleCard } from "/web/cards.js";
import { generation } from "/web/page-state.js";

// One person in a title's credits: their name, linking to their page, and what they did.
function personItem(name, personId, what) {
  return h(
    "li",
    {},
    h("span", { class: "path" }, h("a", { href: `#/people/${personId}` }, name)),
    what ? h("span", { class: "size" }, what) : null,
  );
}

// A title's collection, cast and crew, from its credits; null when there are none.
export function creditsSection(credits) {
  const { cast, crew, collection } = credits;
  if (!cast.length && !crew.length && !collection) return null;
  return h(
    "section",
    { "aria-labelledby": "credits-head" },
    h("h2", { id: "credits-head" }, "Cast and crew"),
    collection
      ? h("p", {}, "Part of ", h("a", { href: `#/collections/${collection.id}` }, collection.name), ".")
      : null,
    crew.length ? h("h3", {}, "Made by") : null,
    crew.length ? h("ul", { class: "files" }, crew.map((c) => personItem(c.name, c.person_id, c.job))) : null,
    cast.length ? h("h3", {}, "Cast") : null,
    cast.length ? h("ul", { class: "files" }, cast.map((c) => personItem(c.name, c.person_id, c.character))) : null,
  );
}

// A person's page: their picture and their titles in the library, with what they did in each.
export async function personPage(id, gen) {
  show("Person", loading("this person"));
  const p = await api(`/people/${id}`);
  if (gen !== generation) return;
  const roles = (t) => [...t.characters.map((c) => `as ${c}`), ...t.jobs].join(", ");
  show(
    p.name,
    h(
      "div",
      { class: "title-head" },
      p.profile ? h("div", { class: "poster" }, h("img", { src: p.profile, alt: "" })) : null,
      h(
        "div",
        { class: "facts" },
        h("h1", {}, p.name),
        metaRow([plural(p.titles.length, "title in the library", "titles in the library")]),
      ),
    ),
    p.titles.length
      ? h(
          "ul",
          { class: "grid" },
          p.titles.map((t) => {
            const card = titleCard(t.kind === "movie" ? "movies" : "series", t);
            const what = roles(t);
            if (what) card.querySelector(".card").append(h("div", { class: "meta" }, what));
            return card;
          }),
        )
      : h("p", { class: "status" }, "None of their titles is in the library now."),
  );
}

const STATES = { requested: "Requested", requestable: "Not in the library" };

// A collection's page: its films in release order, those in the library linking to them.
export async function collectionPage(id, gen) {
  show("Collection", loading("the collection"));
  const c = await api(`/collections/${id}`);
  if (gen !== generation) return;
  const film = (item) => {
    const facts = [h("div", { class: "title" }, item.title), item.year ? h("div", { class: "meta" }, String(item.year)) : null];
    if (item.state === "playable") {
      return h("li", {}, h("a", { class: "card", href: `#/movies/${item.library_id}` }, art(item.poster), ...facts));
    }
    return h("li", {}, h("div", { class: "card" }, art(item.poster), ...facts, h("div", { class: "meta" }, STATES[item.state] ?? "")));
  };
  show(
    c.name,
    h("h1", {}, c.name),
    c.all_films ? null : h("p", { class: "note" }, "Only the films in the library are shown."),
    h("ul", { class: "grid" }, c.items.map(film)),
  );
}
