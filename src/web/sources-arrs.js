// Sonarr, Radarr and Prowlarr: the address and API key an import asks for, and importing
// Sonarr's and Radarr's wanted lists. Admins only.

import { field, form, group } from "/web/forms.js";
import { api, h, plural } from "/web/lib.js";

// URL and API key of a Sonarr, Radarr or Prowlarr, for one import; the key is never stored.
export function arrFields(prefix, app, port) {
  return [
    field(`${prefix}-url`, `${app} address`, h("input", { type: "url", autocomplete: "off", spellcheck: "false", placeholder: `http://192.168.1.10:${port}` }), `As the server reaches it.`),
    field(`${prefix}-key`, "API key", h("input", { type: "password", autocomplete: "off", spellcheck: "false" }), `In ${app} under Settings, General. Used for this import only, never stored.`),
  ];
}

export function arrBody([url, key]) {
  const body = { url: url.input.value.trim(), api_key: key.input.value.trim() };
  if (!/^https?:\/\/.+/.test(body.url)) {
    url.fail("Enter an address starting with http:// or https://.");
    return null;
  }
  if (!body.api_key) {
    key.fail("Enter the API key.");
    return null;
  }
  return body;
}

// `ctx.kind` ("movies" or "series") narrows a page to that kind's profiles, lists and imports,
// as the admin areas show them; without it the page shows both, as Settings did.
const SOURCE_OF = { movies: "radarr", series: "sonarr" };
export const sourcesFor = (ctx) => (ctx.kind ? [SOURCE_OF[ctx.kind]] : ["radarr", "sonarr"]);

export async function arrs(ctx) {
  if (!ctx.current()) return [];
  const make = (source) => {
    const app = source === "radarr" ? "Radarr" : "Sonarr";
    const fields = arrFields(`${source}-list`, app, source === "radarr" ? 7878 : 8989);
    return form(fields, `Import ${app}'s list`, async () => {
      const body = arrBody(fields);
      if (!body) return null;
      const r = await api(`/import/${source}`, { method: "POST", body });
      fields[1].input.value = "";
      const titles = source === "radarr" ? plural(r.movies, "movie", "movies") : plural(r.series, "series", "series");
      return [
        `Imported ${titles}, ${r.monitored} monitored, ${r.on_disk} already in a library.`,
        r.without_library ? `${r.without_library} had a root folder no library matched.` : "",
        r.removed ? `Removed ${r.removed} ${app} no longer lists.` : "",
      ]
        .filter(Boolean)
        .join(" ");
    });
  };
  const groups = {
    radarr: group(
      "Import from Radarr",
      h("p", { class: "note" }, "Its movies join the wanted list. Running it again updates them and removes ones Radarr no longer lists; titles added another way are left alone. Root folders go to the library named like them, or the only movies library."),
      make("radarr"),
    ),
    sonarr: group(
      "Import from Sonarr",
      h("p", { class: "note" }, "Its series join the wanted list, with their monitored seasons. Running it again updates them and removes ones Sonarr no longer lists; titles added another way are left alone."),
      make("sonarr"),
    ),
  };
  return sourcesFor(ctx).map((source) => groups[source]);
}
