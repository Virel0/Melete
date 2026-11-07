// Who's in a title and who made it, a person's titles in the library, and a franchise's films.

import { SignedOut, api, h, loading, metaRow, plural, show } from "/web/lib.js";
import { art, titleCard } from "/web/cards.js";
import { generation, signIn } from "/web/page-state.js";
import { hold, release } from "/web/page-parts.js";

// "Erin Van Der Test" → "ET", "Dee" → "D": the letters standing in for a missing picture.
function initials(name) {
  const words = name.split(/\s+/).filter(Boolean);
  const first = (word) => [...(word ?? "")][0] ?? "";
  return (first(words[0]) + (words.length > 1 ? first(words.at(-1)) : "")).toUpperCase() || "?";
}

// A small round picture of someone in the credits, read only as it nears the screen; their
// initials when there's none, or it can't be read. The name beside it says who it is.
function face(name, src) {
  const letters = () => h("span", { class: "avatar avatar-credit avatar-none", "aria-hidden": "true" }, initials(name));
  if (!src) return letters();
  const img = h("img", { class: "avatar avatar-credit", src, alt: "", loading: "lazy", decoding: "async" });
  img.addEventListener("error", () => img.replaceWith(letters()), { once: true });
  return img;
}

// One person in a title's credits: their picture and name, linking to their page, and what
// they did.
function personItem(name, personId, what, profile) {
  return h(
    "li",
    {},
    h("span", { class: "path" }, h("a", { class: "credit", href: `#/people/${personId}` }, face(name, profile), h("span", { class: "credit-name" }, name))),
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
    crew.length ? h("ul", { class: "files credits" }, crew.map((c) => personItem(c.name, c.person_id, c.job, c.profile))) : null,
    cast.length ? h("h3", {}, "Cast") : null,
    cast.length ? h("ul", { class: "files credits" }, cast.map((c) => personItem(c.name, c.person_id, c.character, c.profile))) : null,
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

// A collection's page: its films in release order, those in the library linking to them, and
// Ask for it under those it hasn't that nobody has asked for.
export async function collectionPage(id, gen) {
  show("Collection", loading("the collection"));
  const c = await api(`/collections/${id}`);
  if (gen !== generation) return;
  // The outcome is said in place, the film's state follows, and focus goes to the link that
  // takes the button's place.
  const askFor = (item, state) => {
    const said = h("p", { class: "meta", role: "status" });
    const button = h("button", { type: "button", class: "quiet", "aria-label": `Ask for ${item.title}` }, "Ask for it");
    button.addEventListener("click", async () => {
      if (!hold(button)) return;
      said.textContent = "Asking…";
      try {
        const asked = await api("/requests", { method: "POST", body: { kind: "movie", tmdb_id: item.tmdb_id } });
        said.textContent =
          asked.state === "approved" ? `Approved: ${asked.title} is on its way.` : `Asked for ${asked.title}. An admin will decide.`;
        state.textContent = STATES.requested;
        const mine = h("a", { href: "#/requests/mine" }, "See your requests");
        button.replaceWith(mine);
        mine.focus();
      } catch (err) {
        if (err instanceof SignedOut) return signIn("Your session ended. Sign in again.");
        said.textContent = err.message;
        release(button);
      }
    });
    return h("div", { class: "card-ask" }, button, said);
  };
  const film = (item) => {
    const facts = [h("div", { class: "title" }, item.title), item.year ? h("div", { class: "meta" }, String(item.year)) : null];
    if (item.state === "playable") {
      return h("li", {}, h("a", { class: "card", href: `#/movies/${item.library_id}` }, art(item.poster), ...facts));
    }
    const state = h("div", { class: "meta" }, STATES[item.state] ?? "");
    const card = h("div", { class: "card" }, art(item.poster), ...facts, state);
    return h("li", {}, card, item.state === "requestable" ? askFor(item, state) : null);
  };
  show(
    c.name,
    h("h1", {}, c.name),
    c.all_films ? null : h("p", { class: "note" }, "Only the films in the library are shown."),
    h("ul", { class: "grid" }, c.items.map(film)),
  );
}
