// The admins' request pages: deciding on everyone's requests, and where approved ones go.

import { field, form, group } from "/web/forms.js";
import { SignedOut, api, h } from "/web/lib.js";
import { importRow } from "/web/requests-imports.js";
import { requestRow } from "/web/requests-row.js";

// ---------- To decide (admins) ----------

// Which libraries a kind of request may go to.
const LIBRARY_KIND = { movie: "movies", series: "shows", artist: "music", album: "music", song: "music" };

// `ctx.requestsService` (movies, series, anime or music), when given, narrows both pages to
// that Manage service's requests and defaults.
export async function decide(ctx) {
  const only = ctx.requestsService ? `&service=${ctx.requestsService}` : "";
  const [page, declined, libraries, profiles, musicProfiles, set] = await Promise.all([
    api(`/requests?all=true&state=requested&limit=200${only}`),
    api(`/requests?all=true&state=declined&limit=50${only}`),
    api("/libraries"),
    api("/profiles"),
    api("/music/profiles"),
    api("/requests/settings"),
  ]);
  if (!ctx.current()) return [];
  const rows = page.items.map((r) => {
    const row = requestRow(ctx, r, { decide: true });
    const actions = row.querySelector(".row-actions");
    const said = h("span", { class: "note", role: "status" });
    const music = LIBRARY_KIND[r.kind] === "music";
    const library = h(
      "select",
      { id: `approve-library-${r.id}`, class: "compact" },
      h("option", { value: "" }, "As set under Where they go"),
      libraries.filter((l) => l.kind === LIBRARY_KIND[r.kind]).map((l) => h("option", { value: l.id }, l.name)),
    );
    const choices = music ? musicProfiles : profiles.filter((p) => p.service === r.service);
    // With several profiles and none set for its kind, approving asks for one: without it,
    // nothing would be grabbed. With one, that one is used.
    const mustChoose = !music && choices.length > 1 && !set.defaults.find((d) => d.kind === r.service)?.profile_id;
    const profile = h(
      "select",
      { id: `approve-profile-${r.id}`, class: "compact" },
      h("option", { value: "" }, mustChoose ? "Choose one" : "As set under Where they go"),
      choices.map((p) => h("option", { value: p.id }, p.name)),
    );
    const approve = h("button", { type: "button" }, "Approve");
    approve.addEventListener("click", async () => {
      if (mustChoose && !profile.value) {
        choose.open = true;
        said.textContent = `Choose a profile: none is set for these requests under Where they go, so it wouldn't be grabbed.`;
        return profile.focus();
      }
      const body = {};
      if (library.value) body.library_id = Number(library.value);
      if (profile.value) body[music ? "music_profile_id" : "profile_id"] = Number(profile.value);
      approve.disabled = true;
      said.textContent = "Approving…";
      try {
        await api(`/requests/${r.id}/approve`, { method: "POST", body });
        ctx.reload(`Approved ${r.title}; it's being looked for.`);
      } catch (err) {
        if (err instanceof SignedOut) return ctx.signedOut();
        said.textContent = err.message;
        approve.disabled = false;
        // Most often there's nowhere set for it to go: the choice is right here.
        choose.open = true;
      }
    });
    const decline = h("button", { type: "button", class: "quiet" }, "Decline");
    decline.addEventListener("click", () => declining(ctx, r, actions));
    const choose = h(
      "details",
      {},
      h("summary", {}, "Choose where it goes"),
      h("div", { class: "search-row" }, h("label", { for: library.id }, "Library"), library, h("label", { for: profile.id }, "Profile"), profile),
    );
    actions.append(approve, decline, said);
    row.firstChild.append(choose);
    return row;
  });
  const playlists = (await api("/requests/imports?all=true")).filter((i) => i.waiting > 0);
  if (!ctx.current()) return [];
  return [
    playlists.length
      ? group(
          "Whole playlists",
          h("p", { class: "note" }, "Approve every track still waiting from one import; they're fetched one after another."),
          h("ul", { class: "rows" }, playlists.map((i) => importRow(ctx, i, { approving: true }))),
        )
      : null,
    group(
      "To decide",
      rows.length ? h("ul", { class: "rows" }, rows) : h("p", { class: "status" }, "Nothing to decide. New requests show up here."),
    ),
    declined.items.length
      ? group(
          "Declined",
          h("p", { class: "note" }, "Declined by mistake? Undoing a decline puts the request back under To decide, and takes away the reason."),
          h("ul", { class: "rows" }, declined.items.map((r) => declinedRow(ctx, r))),
        )
      : null,
  ];
}

// A declined request, with a button that undoes the decline.
function declinedRow(ctx, r) {
  const row = requestRow(ctx, r, { decide: true });
  const actions = row.querySelector(".row-actions");
  const said = h("span", { class: "note", role: "status" });
  const undo = h("button", { type: "button", class: "quiet" }, "Undo decline");
  undo.addEventListener("click", async () => {
    undo.disabled = true;
    said.textContent = "Undoing…";
    try {
      await api(`/requests/${r.id}/reopen`, { method: "POST" });
      ctx.reload(`${r.title}, asked for by ${r.username}, is waiting for a decision again.`);
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      said.textContent = err.message;
      undo.disabled = false;
    }
  });
  actions.append(undo, said);
  return row;
}

// Swaps a request's actions for a reason and a button that declines it.
function declining(ctx, r, actions) {
  const before = [...actions.childNodes];
  const id = `decline-${r.id}`;
  const reason = h("input", { id, autocomplete: "off", maxlength: 500 });
  const said = h("span", { class: "note", role: "status" });
  const go = h("button", { type: "button", class: "danger" }, "Decline request");
  const cancel = h("button", { type: "button", class: "quiet" }, "Cancel");
  cancel.addEventListener("click", () => {
    actions.replaceChildren(...before);
    before.find((n) => n.tagName === "BUTTON")?.focus();
  });
  go.addEventListener("click", async () => {
    if (!reason.value.trim()) {
      said.textContent = `Say why, so ${r.username} knows.`;
      reason.setAttribute("aria-invalid", "true");
      reason.focus();
      return;
    }
    go.disabled = cancel.disabled = true;
    try {
      await api(`/requests/${r.id}/decline`, { method: "POST", body: { reason: reason.value.trim() } });
      ctx.reload(`Declined ${r.title}.`);
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      said.textContent = err.message;
      go.disabled = cancel.disabled = false;
    }
  });
  actions.replaceChildren(h("label", { for: id, class: "question" }, `Why not? ${r.username} will see this.`), reason, go, cancel, said);
  reason.focus();
}

// ---------- Where they go (admins) ----------

const DEFAULTS = [
  ["movies", "Movies", "movies"],
  ["series", "Series", "shows"],
  ["anime", "Anime series", "shows"],
  ["music", "Music", "music"],
];

export async function settings(ctx) {
  const [set, libraries, profiles, musicProfiles, browsed] = await Promise.all([
    api("/requests/settings"),
    api("/libraries"),
    api("/profiles"),
    api("/music/profiles"),
    api("/browse/libraries"),
  ]);
  if (!ctx.current()) return [];
  // With nothing set, a library's category decides when exactly one fits (anime, or none).
  const fitting = (libraryKind, category) => {
    const ofKind = browsed.filter((l) => l.kind === libraryKind);
    const found = ofKind.filter((l) => l.category === category);
    return ofKind.length > 1 && found.length === 1 ? found[0] : null;
  };
  const forms = DEFAULTS.filter(([kind]) => !ctx.requestsService || kind === ctx.requestsService).map(([kind, name, libraryKind]) => {
    const now = set.defaults.find((d) => d.kind === kind) ?? {};
    const music = kind === "music";
    const choices = music ? musicProfiles : profiles.filter((p) => p.service === kind);
    const fits = music ? null : fitting(libraryKind, kind === "anime" ? "anime" : null);
    const fallback = fits
      ? `${fits.name}, the ${kind === "anime" ? "anime library" : "one without a category"}`
      : kind === "anime"
        ? "Where series go"
        : `The only ${libraryKind === "shows" ? "shows" : libraryKind} library`;
    const library = field(
      `defaults-${kind}-library`,
      "Library",
      h(
        "select",
        {},
        h("option", { value: "" }, fallback),
        libraries.filter((l) => l.kind === libraryKind).map((l) => h("option", { value: l.id, selected: l.id === now.library_id }, l.name)),
      ),
    );
    const chosen = music ? now.music_profile_id : now.profile_id;
    const profile = field(
      `defaults-${kind}-profile`,
      "Quality profile",
      h(
        "select",
        {},
        h("option", { value: "" }, music ? "FLAC preferred (the preset)" : choices.length === 1 ? `${choices[0].name}, the only one` : "None chosen"),
        choices.map((p) => h("option", { value: p.id, selected: p.id === chosen }, p.name)),
      ),
    );
    const save = form([library, profile], `Save ${name.toLowerCase()}`, async () => {
      const id = (input) => (input.value ? Number(input.value) : null);
      const body = { library_id: id(library.input), [music ? "music_profile_id" : "profile_id"]: id(profile.input) };
      await api(`/requests/settings/defaults/${kind}`, { method: "PUT", body });
      return "Saved.";
    });
    return h("section", { class: "subgroup", "aria-labelledby": `defaults-${kind}` }, h("h3", { id: `defaults-${kind}` }, name), save);
  });

  const users = set.users.map((u) => {
    if (u.is_admin) return h("li", {}, h("div", {}, h("strong", {}, u.username), h("p", { class: "note" }, "An admin: requests go straight through.")));
    const id = `through-${u.id}`;
    const box = h("input", { type: "checkbox", id, checked: u.requests_approved });
    const said = h("span", { class: "note", role: "status" });
    box.addEventListener("change", async () => {
      box.disabled = true;
      try {
        await api(`/requests/settings/users/${u.id}`, { method: "PUT", body: { requests_approved: box.checked } });
        said.textContent = "Saved.";
      } catch (err) {
        if (err instanceof SignedOut) return ctx.signedOut();
        box.checked = !box.checked;
        said.textContent = err.message;
      } finally {
        box.disabled = false;
      }
    });
    return h(
      "li",
      {},
      h("div", {}, h("strong", {}, u.username)),
      h("div", { class: "row-actions check" }, box, h("label", { for: id }, "Requests go straight through"), said),
    );
  });

  return [
    group(
      "Where approved requests go",
      h("p", { class: "note" }, "Used when an admin approves without choosing, and for requests that go straight through. Anime series go where series do unless set here."),
      ...forms,
    ),
    group("Whose requests go straight through", h("ul", { class: "rows" }, users)),
  ];
}
