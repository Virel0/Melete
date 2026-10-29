// Finding subtitles a file lacks on OpenSubtitles, from the player: pick a language, see what
// there is, and fetch one, which then plays like the file's own. Shown only once an admin has
// set OpenSubtitles up.

import { SignedOut, announce, api, h, languageName, plural } from "/web/lib.js";

// The languages offered, as OpenSubtitles names them.
const LANGUAGES = [
  "ar", "cs", "da", "de", "el", "en", "es", "fi", "fr", "he", "hu", "it", "ja", "ko", "nl", "no",
  "pl", "pt", "pt-br", "ro", "ru", "sv", "tr", "uk", "zh-cn", "zh-tw",
];

// The browser's language when it's one of those, else English.
function preferred() {
  const asked = (navigator.language ?? "en").toLowerCase();
  return LANGUAGES.find((l) => l === asked) ?? LANGUAGES.find((l) => l === asked.split("-")[0]) ?? "en";
}

// The panel; `use(track)` adds a fetched track to the player and shows it.
export function subtitleSearch(fileId, use) {
  const language = h(
    "select",
    { id: "subtitle-language" },
    LANGUAGES.map((l) => h("option", { value: l, selected: l === preferred() }, languageName(l) ?? l)),
  );
  const status = h("p", { class: "status", role: "status" });
  const results = h("ul", { class: "subtitle-results" });
  const search = h("button", { type: "submit" }, "Search");
  const form = h(
    "form",
    { class: "toolbar" },
    h("div", {}, h("label", { for: "subtitle-language" }, "Language"), language),
    search,
  );
  const context = { fileId, use, status, results, language };
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    search.disabled = true;
    find(context).finally(() => (search.disabled = false));
  });
  return h(
    "details",
    { class: "subtitle-search" },
    h("summary", {}, "Find subtitles online"),
    h("p", { class: "muted" }, "From OpenSubtitles. Only this video's fingerprint, its title's id and the language are sent."),
    form,
    status,
    results,
  );
}

// Searches and lists what was found.
async function find(c) {
  const code = c.language.value;
  const name = languageName(code) ?? code;
  c.results.replaceChildren();
  c.status.textContent = `Searching for ${name} subtitles…`;
  let found;
  try {
    found = await api(`/files/${c.fileId}/subtitles/search?language=${encodeURIComponent(code)}`, { method: "POST" });
  } catch (err) {
    if (err instanceof SignedOut) throw err;
    c.status.textContent = err.message;
    return;
  }
  if (found.fetched) {
    c.status.textContent = `${name} subtitles were fetched for this video already.`;
    c.results.append(h("li", {}, h("span", {}, found.fetched.title ?? name), useButton(c, () => Promise.resolve(found.fetched))));
    return;
  }
  if (!found.results.length) {
    c.status.textContent = `OpenSubtitles has no ${name} subtitles for this video.${found.remembered ? " It's asked again after a week." : ""}`;
    return;
  }
  c.status.textContent = `${plural(found.results.length, "result", "results")} in ${name}.`;
  for (const r of found.results) {
    const facts = [r.matches_file ? "made for this very file" : null, r.hearing_impaired ? "SDH" : null, plural(r.downloads, "download", "downloads")];
    const fetch = () => api(`/files/${c.fileId}/subtitles/fetch`, { method: "POST", body: { language: code, result: r.id } });
    c.results.append(
      h("li", {}, h("span", {}, r.release ?? "Unnamed release", h("small", {}, ` ${facts.filter(Boolean).join(", ")}`)), useButton(c, fetch)),
    );
  }
}

// A button fetching a result (or taking the one fetched) and showing it.
function useButton(c, fetch) {
  const button = h("button", { type: "button", class: "quiet" }, "Use these");
  button.addEventListener("click", async () => {
    button.disabled = true;
    c.status.textContent = "Fetching…";
    try {
      const track = await fetch();
      c.use(track);
      c.status.textContent = "Fetched, and showing now.";
      announce("Subtitles fetched and shown");
    } catch (err) {
      if (err instanceof SignedOut) throw err;
      c.status.textContent = err.message;
      button.disabled = false;
    }
  });
  return button;
}
