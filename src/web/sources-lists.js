// Import lists: TMDB, IMDb and Rotten Tomatoes lists whose titles join the wanted list, with
// adding, editing, syncing and removing them. Admins only.

import { confirmInline, field, form, group, when } from "/web/forms.js";
import { SignedOut, announce, api, h } from "/web/lib.js";

const LIST_HINTS = {
  tmdb: "popular, top_rated, trending_day, trending_week, now_playing or upcoming for movies; on_the_air or airing_today for series; or list: and a TMDB list's number.",
  imdb: "top, popular, or a public list's id (ls and digits).",
  rottentomatoes: "A https://www.rottentomatoes.com/browse/... page.",
};
const PROVIDERS = { tmdb: "TMDB", imdb: "IMDb", rottentomatoes: "Rotten Tomatoes" };

function listResult(l) {
  const r = l.last_result;
  if (!r) return "Not synced yet.";
  if (r.error) return `Last sync failed: ${r.error}`;
  return `Last synced ${when(l.synced_at)}: ${r.found} found, ${r.added} added, ${r.already_wanted} already wanted${r.skipped.length ? `, ${r.skipped.length} skipped` : ""}.`;
}

// A list as PUT /lists/{id} takes it.
function inputOf(l) {
  return {
    name: l.name,
    provider: l.provider,
    list: l.list,
    kind: l.kind,
    library_id: l.library_id,
    monitored: l.monitored,
    quality_profile: l.quality_profile,
    max_items: l.max_items,
    monitor_seasons: l.monitor_seasons,
    enabled: l.enabled,
  };
}

export async function lists(ctx) {
  const [all, libraries, profilesAll] = await Promise.all([api("/lists"), api("/libraries"), api("/profiles")]);
  if (!ctx.current()) return [];
  // What the rows share: the libraries and profiles to choose from, and the list they're in.
  const page = {
    ctx,
    libraries,
    profiles: profilesAll,
    rows: h("ul", { class: "rows" }),
    mine: (items) => (ctx.kind ? items.filter((l) => l.kind === ctx.kind) : items),
  };
  draw(page, all);

  const add = listForm({
    prefix: "list",
    kind: ctx.kind,
    libraries,
    profiles: profilesAll,
    button: "Add list",
    send: async (body) => {
      await api("/lists", { method: "POST", body: { ...body, enabled: true } });
      await refresh(page);
      return `Added ${body.name}. It syncs every 12 hours; Sync now reads it at once.`;
    },
  });

  return [
    group(
      "Import lists",
      h("p", { class: "note" }, "Their titles join the wanted list every 12 hours. Nothing is ever removed from the wanted list. Syncing needs MELETE_TMDB_TOKEN."),
      page.mine(all).length ? null : h("p", { class: "status" }, "No lists yet."),
      page.rows,
    ),
    group("Add a list", add),
  ];
}

// Asks for the lists again and redraws them.
async function refresh(page) {
  draw(page, await api("/lists"));
}

// Draws the area's lists.
function draw(page, items) {
  page.rows.replaceChildren(...page.mine(items).map((l) => listRow(page, l)));
}

// One import list: what it adds and how its last sync went, with Sync now, Edit, Turn off (or
// on) and Remove.
function listRow(page, l) {
  const { ctx, libraries } = page;
  const libraryName = (id) => libraries.find((x) => x.id === id)?.name ?? `library ${id}`;
  const status = h("p", { class: "note", role: "status" }, listResult(l));
  const actions = h("div", { class: "row-actions" });
  const sync = h("button", { type: "button", class: "quiet" }, "Sync now");
  sync.addEventListener("click", async () => {
    sync.disabled = true;
    status.textContent = "Syncing…";
    try {
      await api(`/lists/${l.id}/sync`, { method: "POST" });
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      // A failed read is kept as the list's last result; a refusal isn't.
      if (err.status !== 502) {
        status.textContent = err.message;
        sync.disabled = false;
        return;
      }
    }
    await refresh(page);
  });
  const toggle = h("button", { type: "button", class: "quiet" }, l.enabled ? "Turn off" : "Turn on");
  toggle.addEventListener("click", async () => {
    toggle.disabled = true;
    try {
      await api(`/lists/${l.id}`, { method: "PUT", body: { ...inputOf(l), enabled: !l.enabled } });
      announce(l.enabled ? `Turned off ${l.name}` : `Turned on ${l.name}`);
      await refresh(page);
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      status.textContent = err.message;
      toggle.disabled = false;
    }
  });
  const remove = h("button", { type: "button", class: "quiet" }, "Remove");
  remove.addEventListener("click", () =>
    confirmInline(actions, {
      question: `Remove “${l.name}”? The titles it added stay wanted.`,
      confirm: "Remove list",
      run: async () => {
        await api(`/lists/${l.id}`, { method: "DELETE" });
        announce(`Removed ${l.name}`);
        await refresh(page);
      },
    }),
  );
  const edit = h("button", { type: "button", class: "quiet", "aria-label": `Edit ${l.name}` }, "Edit");
  actions.append(sync, edit, toggle, remove);
  const summary = h(
    "div",
    {},
    h("strong", {}, l.name),
    " ",
    h("span", { class: "badge" }, PROVIDERS[l.provider]),
    l.enabled ? null : h("span", { class: "badge" }, "Turned off"),
    h("p", { class: "note path" }, `${l.list}. Up to ${l.max_items} ${l.kind} into ${libraryName(l.library_id)}, with ${l.quality_profile}${l.monitored ? "" : ", not monitored"}.`),
    status,
  );
  const row = h("li", {}, summary, actions);
  edit.addEventListener("click", () => editInRow(page, l, row, [summary, actions], edit));
  return row;
}

// Swaps a row for the form that edits its list; Cancel puts the row back.
function editInRow(page, l, row, shown, edit) {
  const close = () => {
    row.replaceChildren(...shown);
    edit.focus();
  };
  const editor = listForm({
    prefix: `list-${l.id}`,
    list: l,
    libraries: page.libraries,
    profiles: page.profiles,
    button: "Save changes",
    cancel: close,
    send: async (body) => {
      await api(`/lists/${l.id}`, { method: "PUT", body: { ...body, enabled: l.enabled } });
      announce(`Saved ${body.name}`);
      await refresh(page);
      return null;
    },
  });
  row.replaceChildren(h("div", {}, h("h3", {}, `Edit ${l.name}`), editor));
  editor.querySelector("input")?.focus();
}

// The form for a new import list or, given `list`, for changing one. Library and profile
// choices follow the kind of titles. `send(body)` saves it and returns what to say.
function listForm({ prefix, list = null, kind: only = null, libraries, profiles, button, send, cancel = null }) {
  const name = field(`${prefix}-name`, "Name", h("input", { autocomplete: "off", value: list?.name ?? "" }));
  const provider = h("select", {}, Object.entries(PROVIDERS).map(([k, v]) => h("option", { value: k, selected: k === list?.provider }, v)));
  const providerField = field(`${prefix}-provider`, "From", provider);
  const listInput = h("input", { autocomplete: "off", spellcheck: "false", value: list?.list ?? "" });
  const listField = field(`${prefix}-list`, "List", listInput, LIST_HINTS[list?.provider ?? "tmdb"]);
  const kind = h(
    "select",
    {},
    h("option", { value: "movies", selected: (list?.kind ?? only) === "movies" }, "Movies"),
    h("option", { value: "series", selected: (list?.kind ?? only) === "series" }, "Series"),
  );
  const kindField = field(`${prefix}-kind`, "Titles", kind);
  // An area's lists are all of its kind.
  kindField.box.hidden = Boolean(only);
  const library = h("select", {});
  const libraryField = field(`${prefix}-library`, "Library", library);
  const profile = h("select", {});
  const profileField = field(`${prefix}-profile`, "Quality profile", profile);
  const max = h("input", { type: "number", min: 1, max: 250, value: list?.max_items ?? 50, inputmode: "numeric" });
  const maxField = field(`${prefix}-max`, "Most titles", max, "1 to 250, from the top of the list.");
  const seasons = h(
    "select",
    {},
    [["all", "All seasons"], ["latest", "The latest season"], ["none", "No seasons"]].map(([value, label]) =>
      h("option", { value, selected: value === list?.monitor_seasons }, label),
    ),
  );
  const seasonsField = field(`${prefix}-seasons`, "New series monitor", seasons);
  const monitored = h("input", { type: "checkbox", id: `${prefix}-monitored`, checked: list ? list.monitored : true });
  const monitoredBox = { box: h("div", { class: "check" }, monitored, h("label", { for: `${prefix}-monitored` }, "Monitored: searched for, and grabbed when downloads are on")), clear() {} };

  // The profiles of the service the list's titles go to: a series list into a library with the
  // anime category adds Anime's series, which take Anime's.
  const followLibrary = () => {
    const k = kind.value;
    const chosen = libraries.find((l) => String(l.id) === library.value);
    const service = k === "movies" ? "movies" : chosen?.category === "anime" ? "anime" : "series";
    const keep = profile.value || list?.quality_profile;
    const profs = profiles.filter((p) => p.service === service);
    profile.replaceChildren(...profs.map((p) => h("option", { value: p.name, selected: p.name === keep }, p.name)));
  };
  const follow = () => {
    const k = kind.value;
    const libs = libraries.filter((l) => l.kind === (k === "movies" ? "movies" : "shows"));
    library.replaceChildren(...libs.map((l) => h("option", { value: l.id, selected: l.id === list?.library_id }, l.name)));
    followLibrary();
    seasonsField.box.hidden = k !== "series";
  };
  kind.addEventListener("change", follow);
  library.addEventListener("change", followLibrary);
  provider.addEventListener("change", () => {
    listField.box.querySelector(".note").textContent = LIST_HINTS[provider.value];
  });
  follow();

  const fields = [name, providerField, listField, kindField, libraryField, profileField, maxField, seasonsField, monitoredBox];
  const el = form(fields, button, async () => {
    const body = {
      name: name.input.value.trim(),
      provider: provider.value,
      list: listInput.value.trim(),
      kind: kind.value,
      library_id: Number(library.value),
      monitored: monitored.checked,
      quality_profile: profile.value,
      max_items: Number(max.value),
      monitor_seasons: seasons.value,
    };
    let bad = false;
    const fail = (f, message) => {
      f.fail(message);
      bad = true;
    };
    if (!body.name) fail(name, "Give the list a name.");
    if (!body.list) fail(listField, "Say which list.");
    if (!library.value) fail(libraryField, `There's no ${body.kind === "movies" ? "movies" : "shows"} library to put them in.`);
    if (!profile.value) fail(profileField, "Import or make a quality profile for these first.");
    if (!(body.max_items >= 1 && body.max_items <= 250)) fail(maxField, "Use a number from 1 to 250.");
    if (bad) return null;
    try {
      const said = await send(body);
      if (!list) name.input.value = listInput.value = "";
      return said;
    } catch (err) {
      if (err.status === 409) {
        name.fail("A list has that name already.");
        return null;
      }
      throw err;
    }
  });
  if (cancel) {
    const back = h("button", { type: "button", class: "quiet" }, "Cancel");
    back.addEventListener("click", cancel);
    el.querySelector(".form-actions").append(back);
  }
  return el;
}
