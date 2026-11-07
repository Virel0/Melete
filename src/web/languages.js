// Your audio and subtitle languages, which every app plays files with, and, for movies, shows or
// anime, languages of their own when you want them (anime with Japanese audio, say): a part of
// Your account.

import { field, form, group } from "/web/forms.js";
import { api, h, languageName } from "/web/lib.js";

const CODE = /^[a-z]{2,3}([-_]([a-z]{2}|\d{3}))?$/i;

const MODES = [
  ["foreign", "When the audio isn't in one of my subtitle languages"],
  ["always", "Always"],
  ["forced", "Only forced subtitles (signs and lines in another language)"],
  ["off", "Never"],
];

// The codes typed in a field, or null after saying which one isn't a code.
function codes(f) {
  const list = f.input.value
    .split(/[\s,]+/)
    .map((c) => c.trim())
    .filter(Boolean);
  const wrong = list.find((c) => !CODE.test(c));
  if (wrong) {
    f.fail(`“${wrong}” isn't a language code. Use codes such as en, jpn or pt-BR.`);
    return null;
  }
  if (list.length > 10) {
    f.fail("List at most 10 languages.");
    return null;
  }
  return list;
}

const names = (list) => (list.length ? list.map((c) => languageName(c)).join(", ") : "none");

// The three fields of a set of languages, their ids starting with `prefix`, filled from `saved`.
function languageFields(prefix, saved) {
  const hint = "Language codes, most wanted first, separated by commas: for example ja, en, pt-BR.";
  const audio = field(`${prefix}audio-languages`, "Audio languages", h("input", { autocomplete: "off", spellcheck: "false", value: saved.audio_languages.join(", ") }), hint);
  const subtitles = field(
    `${prefix}subtitle-languages`,
    "Subtitle languages",
    h("input", { autocomplete: "off", spellcheck: "false", value: saved.subtitle_languages.join(", ") }),
    `${hint} Audio in one of these counts as understood.`,
  );
  const mode = field(
    `${prefix}subtitle-mode`,
    "Show subtitles",
    h(
      "select",
      {},
      MODES.map(([value, label]) => h("option", { value, selected: value === saved.subtitle_mode }, label)),
    ),
  );
  return { audio, subtitles, mode };
}

// The codes of a set's fields as the API takes them, or null after a field says what's wrong.
function body({ audio, subtitles, mode }) {
  const audioCodes = codes(audio);
  const subtitleCodes = codes(subtitles);
  if (!audioCodes || !subtitleCodes) return null;
  return { audio_languages: audioCodes, subtitle_languages: subtitleCodes, subtitle_mode: mode.input.value };
}

function fill({ audio, subtitles, mode }, kept) {
  audio.input.value = kept.audio_languages.join(", ");
  subtitles.input.value = kept.subtitle_languages.join(", ");
  mode.input.value = kept.subtitle_mode;
}

const KINDS = [
  ["movies", "Movies"],
  ["shows", "Shows"],
  ["anime", "Anime"],
];

export async function languagesGroup() {
  const saved = await api("/me/languages");
  const fields = languageFields("", saved);
  const saving = form([fields.audio, fields.subtitles, fields.mode], "Save languages", async () => {
    const asked = body(fields);
    if (!asked) return null;
    const kept = await api("/me/languages", { method: "PUT", body: asked });
    fill(fields, kept);
    return `Saved. Audio: ${names(kept.audio_languages)}. Subtitles: ${names(kept.subtitle_languages)}.`;
  });
  const kinds = await Promise.all(KINDS.map(([kind, label]) => kindForm(kind, label)));
  return group(
    "Languages",
    h(
      "p",
      { class: "note" },
      "Every app plays with these. In a series, the audio and subtitles you choose by hand are used for its next episodes instead.",
    ),
    saving,
    h("h3", {}, "Movies, shows and anime"),
    h("p", { class: "note" }, "Each follows the languages above unless you give it its own: anime with Japanese audio and English subtitles, say."),
    kinds,
  );
}

// A kind's own languages: a box to have them, the three fields while it's ticked, and Save.
async function kindForm(kind, label) {
  const saved = await api(`/me/languages/${kind}`);
  const id = `${kind}-own-languages`;
  const own = h("input", { type: "checkbox", id, checked: saved.own });
  const ownBox = { box: h("div", { class: "check" }, own, h("label", { for: id }, `${label} have languages of their own`)), input: own, clear() {} };
  const fields = languageFields(`${kind}-`, saved);
  const parts = [fields.audio, fields.subtitles, fields.mode];
  const showParts = () => {
    for (const part of parts) part.box.hidden = !own.checked;
  };
  own.addEventListener("change", showParts);
  showParts();
  const saving = form([ownBox, ...parts], `Save ${label.toLowerCase()} languages`, async () => {
    if (!own.checked) {
      const kept = await api(`/me/languages/${kind}`, { method: "DELETE" });
      fill(fields, kept);
      return `Saved. ${label} follow your languages.`;
    }
    const asked = body(fields);
    if (!asked) return null;
    const kept = await api(`/me/languages/${kind}`, { method: "PUT", body: asked });
    fill(fields, kept);
    return `Saved. ${label}: audio ${names(kept.audio_languages)}, subtitles ${names(kept.subtitle_languages)}.`;
  });
  return h("section", { class: "kind-languages", "aria-label": `${label} languages` }, saving);
}
