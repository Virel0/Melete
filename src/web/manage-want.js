// Wanting a title that's only in the library, from its page in Manage: a profile, whether to
// search for it now, and Monitor. It joins the wanted list as an admin's add does (a request
// approved at once), staying in the library it's in; then its page opens on the wanted entry,
// with the switches and searches a wanted title has.

import { SignedOut, announce, api, h } from "/web/lib.js";

// `request` is what POST /requests is asked for (the kind and its id), or null when it can't be
// wanted, `why` saying so. `note` says what not being wanted means; `library` is the name of
// the library it's in; `music` takes a music profile. `opened(r)` is the address of its page
// once wanted (or a promise of it; null to stay and say so).
export function wantPanel(ctx, { note, request, why, library, profiles, music, opened }) {
  if (!request) return h("div", { class: "want-box" }, h("p", { class: "note" }, note), h("p", { class: "note" }, why));
  const id = "want";
  const profile = h(
    "select",
    { id: `${id}-profile` },
    h("option", { value: "" }, "As Where they go says"),
    profiles.map((p) => h("option", { value: String(p.id) }, p.name)),
  );
  const now = h("input", { type: "checkbox", id: `${id}-now` });
  const submit = h("button", { type: "submit" }, "Monitor");
  const said = h("p", { class: "form-result", role: "status" });
  const form = h(
    "form",
    { class: "want-form", novalidate: true, "aria-label": "Put it on the wanted list" },
    h("div", { class: "field" }, h("label", { for: profile.id }, music ? "Music profile" : "Profile"), profile),
    h("div", { class: "check" }, now, h("label", { for: now.id }, "Search for what's missing now")),
    h("div", { class: "form-actions" }, submit),
  );
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    submit.disabled = true;
    said.classList.remove("error");
    said.textContent = "Putting it on the wanted list…";
    try {
      const libraries = await api("/libraries");
      const here = libraries.find((l) => l.name === library);
      const body = {
        ...request,
        library_id: here?.id,
        [music ? "music_profile_id" : "profile_id"]: profile.value ? Number(profile.value) : undefined,
        monitored: true,
        search_now: now.checked,
      };
      const r = await api("/requests", { method: "POST", body });
      if (r.state !== "approved" || r.wanted_id === null) {
        said.textContent = "Asked for, but it couldn't be placed: choose where it goes under Requests, To decide.";
        return;
      }
      const where = await opened(r);
      const message = `On the wanted list and monitored.${r.search_started ? " Searching for it now." : ""}`;
      announce(message);
      if (where) location.hash = where;
      else said.textContent = message;
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      said.classList.add("error");
      const outside = err.status === 502 || /isn't set up/.test(err.message);
      said.replaceChildren(...[`${err.message} `, outside ? h("a", { href: "#/admin/system/services" }, "How TMDB and MusicBrainz stand") : null].filter(Boolean));
      submit.disabled = false;
    }
  });
  return h("div", { class: "want-box" }, h("p", { class: "note" }, note), form, said);
}
