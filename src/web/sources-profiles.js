// Quality profiles and custom formats, as lists: opening the editors, trying a release, the
// anime profile, and importing Radarr's and Sonarr's. Admins only.

import { confirmInline, form, group } from "/web/forms.js";
import { api, h, plural } from "/web/lib.js";
import { conditionSummary } from "/web/condition-editor.js";
import { FOR, blankProfile, formatEditor, profileEditor, tryRelease } from "/web/profile-editor.js";
import { arrBody, arrFields, sourcesFor } from "/web/sources-arrs.js";

const appName = (source) => (source === "radarr" ? "Radarr" : "Sonarr");

export async function profiles(ctx) {
  const [all, formats] = await Promise.all([api("/profiles"), api("/custom-formats")]);
  if (!ctx.current()) return [];
  const sources = sourcesFor(ctx);
  // The section shows either its lists or one editor in their place.
  const page = { ctx, all, formats, view: h("div"), listParts: [], opener: null };
  page.back = (message) => (message ? ctx.reload(message) : showList(page));

  const imports = sources.map((source) => importForm(ctx, source));
  const animePlace = ctx.service ? ANIME_PROFILE[ctx.service.key] : { title: "Anime profile", sources };

  const one = sources.length === 1;
  const formatsPart = group(
    "Custom formats",
    h("p", { class: "note" }, "What profiles score releases by: words in the name, languages, sources and the rest."),
    ...sources.map((source) => formatList(page, source)),
  );
  // A Manage service shows its profiles and its custom formats as pages of their own
  // (`ctx.only`: "profiles" or "formats").
  // Series and Anime each have their own profiles, though both score Sonarr's formats.
  const services = ctx.service ? [ctx.service.key] : ["movies", "series", "anime"];
  const parts = [
    ...services.map((service) => group(PROFILES_OF[service], ...profileList(page, service))),
    formatsPart,
    animePlace
      ? group(
          animePlace.title,
          h("p", { class: "note" }, "1080p, upgraded to 2160p when there is one; dual Japanese and English audio first, then English, then Japanese with English subtitles. Raws are refused."),
          animeForm(ctx, animePlace.sources),
        )
      : null,
    ...sources.map((source, i) =>
      group(
        `Import from ${appName(source)}`,
        h("p", { class: "note" }, i === 0 || one ? `Makes Melete's copies of ${appName(source)}'s profiles and formats match it. Profiles made or edited here are left alone.` : "The same, for Sonarr's.", source === "sonarr" ? " Each of Sonarr's profiles goes where its series are: to Anime when only anime series name it, to Series otherwise, and to both when both do." : null),
        imports[i],
      ),
    ),
  ].filter(Boolean);
  page.listParts = ctx.only === "formats" ? [formatsPart] : ctx.only === "profiles" ? parts.filter((p) => p !== formatsPart) : parts;
  page.view.replaceChildren(...page.listParts);
  return [page.view];
}

// Back to the lists, with focus on the button that opened the editor.
function showList(page) {
  page.view.replaceChildren(...page.listParts);
  page.opener?.focus();
}

// Shows an editor in the lists' place; `from` is the button that opened it.
function open(page, from, editor) {
  page.opener = from;
  page.view.replaceChildren(editor);
  editor.scrollIntoView({ block: "start" });
}

// Each service's list of profiles, by its heading.
const PROFILES_OF = { movies: "Movie profiles", series: "Series profiles", anime: "Anime profiles" };

// One service's profiles, with New profile under them.
function profileList(page, service) {
  const { ctx, all, formats } = page;
  const source = service === "movies" ? "radarr" : "sonarr";
  const items = all.filter((p) => p.service === service);
  const make = h("button", { type: "button", class: "quiet" }, `New profile for ${service}`);
  make.addEventListener("click", () => open(page, make, profileEditor({ profile: blankProfile(source, service), formats, done: page.back, ctx })));
  if (!items.length) return [h("p", { class: "status" }, `None yet. Make one, or import ${appName(source)}'s below.`), h("div", { class: "form-actions" }, make)];
  const list = h(
    "ul",
    { class: "rows" },
    items.map((p) => profileRow(page, source, p)),
  );
  return [list, h("div", { class: "form-actions" }, make)];
}

// A profile: what it allows and scores, and Edit, Copy, Try a release and Delete.
function profileRow(page, source, p) {
  const { ctx, formats } = page;
  const allowed = p.items.filter((i) => i.allowed).map((i) => i.name);
  const scored = [...p.formats].sort((a, b) => b.score - a.score);
  const actions = h("div", { class: "row-actions" });
  const info = h(
    "div",
    {},
    h("strong", {}, p.name),
    " ",
    h("span", { class: "badge" }, p.origin === "melete" ? "Made here" : `From ${appName(source)}`),
    h("p", {}, `Up to ${p.cutoff}${p.upgrade_allowed ? ", upgrading until then" : ", no upgrades"}.`),
    h("p", { class: "note" }, `Allowed: ${allowed.join(", ") || "nothing"}.`),
    scored.length
      ? h("p", { class: "note" }, `Formats: ${scored.slice(0, 6).map((f) => `${f.name} ${f.score > 0 ? "+" : ""}${f.score}`).join(", ")}${scored.length > 6 ? `, and ${scored.length - 6} more` : ""}. Turned down below ${p.min_format_score}.`)
      : null,
  );
  const edit = h("button", { type: "button", class: "quiet", "aria-label": `Edit ${p.name}` }, "Edit");
  edit.addEventListener("click", () => open(page, edit, profileEditor({ profile: p, formats, done: page.back, ctx })));
  const copy = h("button", { type: "button", class: "quiet", "aria-label": `Copy ${p.name}` }, "Copy");
  copy.addEventListener("click", () => {
    const { id: _id, origin: _origin, ...rest } = p;
    open(page, copy, profileEditor({ profile: { ...rest, name: `${p.name} (copy)` }, formats, done: page.back, ctx }));
  });
  let trying = null;
  const tryIt = h("button", { type: "button", class: "quiet", "aria-expanded": "false", "aria-label": `Try a release with ${p.name}` }, "Try a release");
  tryIt.addEventListener("click", () => {
    if (trying) {
      trying.remove();
      trying = null;
      tryIt.setAttribute("aria-expanded", "false");
      return;
    }
    trying = tryRelease(p, ctx);
    info.append(trying);
    tryIt.setAttribute("aria-expanded", "true");
    trying.querySelector("input").focus();
  });
  const remove = h("button", { type: "button", class: "quiet", "aria-label": `Delete ${p.name}` }, "Delete");
  remove.addEventListener("click", () =>
    confirmInline(actions, {
      question: `Delete ${p.name}? Wanted titles that chose it go back to the profile ${appName(source)} gave them.`,
      confirm: "Delete profile",
      run: async () => {
        await api(`/profiles/${p.id}`, { method: "DELETE" });
        ctx.reload(`Deleted ${p.name}.`);
      },
    }),
  );
  actions.append(edit, copy, tryIt, remove);
  return h("li", {}, info, actions);
}

// One kind's custom formats, each with Edit and Delete, and New format under them.
function formatList(page, source) {
  const { ctx, formats } = page;
  const items = formats.filter((f) => f.source === source);
  const make = h("button", { type: "button", class: "quiet" }, `New format for ${FOR[source]}`);
  make.addEventListener("click", () => open(page, make, formatEditor({ format: { source, name: "", conditions: [] }, done: page.back, ctx })));
  const list = items.length
    ? h(
        "ul",
        { class: "rows" },
        items.map((f) => {
          const actions = h("div", { class: "row-actions" });
          const edit = h("button", { type: "button", class: "quiet", "aria-label": `Edit ${f.name}` }, "Edit");
          edit.addEventListener("click", () => open(page, edit, formatEditor({ format: f, done: page.back, ctx })));
          const remove = h("button", { type: "button", class: "quiet", "aria-label": `Delete ${f.name}` }, "Delete");
          remove.addEventListener("click", () =>
            confirmInline(actions, {
              question: `Delete ${f.name}? Profiles stop scoring it.`,
              confirm: "Delete format",
              run: async () => {
                await api(`/custom-formats/${f.id}`, { method: "DELETE" });
                ctx.reload(`Deleted ${f.name}.`);
              },
            }),
          );
          actions.append(edit, remove);
          return h(
            "li",
            {},
            h("div", {}, h("strong", {}, f.name), " ", h("span", { class: "badge" }, f.origin === "melete" ? "Made here" : `From ${appName(source)}`), h("p", { class: "note" }, conditionSummary(f))),
            actions,
          );
        }),
      )
    : h("p", { class: "status" }, `None for ${FOR[source]} yet.`);
  return h("div", { class: "subgroup" }, h("h3", {}, source === "radarr" ? "For movies" : "For series and anime"), list, h("div", { class: "form-actions" }, make));
}

// Importing Radarr's or Sonarr's profiles and custom formats.
function importForm(ctx, source) {
  const app = appName(source);
  const fields = arrFields(`${source}-profiles`, app, source === "radarr" ? 7878 : 8989);
  return form(fields, `Import ${app}'s profiles`, async () => {
    const body = arrBody(fields);
    if (!body) return null;
    const report = await api(`/import/${source}/profiles`, { method: "POST", body });
    ctx.reload(`Imported ${app}'s ${plural(report.profiles, "profile", "profiles")} and ${plural(report.formats, "custom format", "custom formats")}.`);
    return null;
  });
}

// Where each service offers the anime profile: Anime for its series, with the choice of giving
// it to every anime series; Movies for anime films, which have no such choice; Series not at
// all, since anime series are Anime's. Outside a service (Settings), for either kind.
const ANIME_PROFILE = {
  anime: { title: "Anime profile", sources: ["sonarr"] },
  movies: { title: "Anime profile for anime films", sources: ["radarr"] },
  series: null,
};

// Making the anime profile, for Anime's series or for movies, and choosing it for every anime series.
function animeForm(ctx, sources) {
  const animeFor = h("select", { id: "anime-source" }, sources.includes("sonarr") ? h("option", { value: "sonarr" }, "Series") : null, sources.includes("radarr") ? h("option", { value: "radarr" }, "Movies") : null);
  const applyBox = h("input", { type: "checkbox", id: "anime-apply" });
  return form(
    [
      // In an area there's one kind, so there's nothing to choose.
      sources.length > 1 ? { box: h("div", { class: "field" }, h("label", { for: "anime-source" }, "For"), animeFor), clear() {} } : null,
      sources.includes("sonarr") ? { box: h("div", { class: "check" }, applyBox, h("label", { for: "anime-apply" }, "Also choose it for every wanted anime series")), clear() {} } : null,
    ].filter(Boolean),
    "Make the anime profile",
    async () => {
      const source = animeFor.value;
      await api("/profiles/presets/anime", { method: "POST", body: { source, apply_to_anime: source === "sonarr" && applyBox.checked } });
      ctx.reload(`Made the Anime profile for ${source === "sonarr" ? "series" : "movies"}.`);
      return null;
    },
  );
}
