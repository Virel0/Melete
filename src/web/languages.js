// Your audio and subtitle languages, which every app plays files with: a part of Your account.

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

export async function languagesGroup() {
  const saved = await api("/me/languages");
  const hint = "Language codes, most wanted first, separated by commas: for example ja, en, pt-BR.";
  const audio = field("audio-languages", "Audio languages", h("input", { autocomplete: "off", spellcheck: "false", value: saved.audio_languages.join(", ") }), hint);
  const subtitles = field(
    "subtitle-languages",
    "Subtitle languages",
    h("input", { autocomplete: "off", spellcheck: "false", value: saved.subtitle_languages.join(", ") }),
    `${hint} Audio in one of these counts as understood.`,
  );
  const mode = field(
    "subtitle-mode",
    "Show subtitles",
    h(
      "select",
      {},
      MODES.map(([value, label]) => h("option", { value, selected: value === saved.subtitle_mode }, label)),
    ),
  );
  const saving = form([audio, subtitles, mode], "Save languages", async () => {
    const audioCodes = codes(audio);
    const subtitleCodes = codes(subtitles);
    if (!audioCodes || !subtitleCodes) return null;
    const kept = await api("/me/languages", {
      method: "PUT",
      body: { audio_languages: audioCodes, subtitle_languages: subtitleCodes, subtitle_mode: mode.input.value },
    });
    audio.input.value = kept.audio_languages.join(", ");
    subtitles.input.value = kept.subtitle_languages.join(", ");
    return `Saved. Audio: ${names(kept.audio_languages)}. Subtitles: ${names(kept.subtitle_languages)}.`;
  });
  return group(
    "Languages",
    h(
      "p",
      { class: "note" },
      "Every app plays with these. In a series, the audio and subtitles you choose by hand are used for its next episodes instead.",
    ),
    saving,
  );
}
