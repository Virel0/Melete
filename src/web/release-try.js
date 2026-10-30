// Manage → Indexers → Try a release name: how Melete reads a name an indexer lists, and, with a
// quality profile chosen, whether that profile would take it and why. Nothing is searched for
// or grabbed; Melete has no free search of the indexers on purpose, as a grab must be a result
// for a wanted title. A release turned down in a search links here with its name and profile.

import { field } from "/web/forms.js";
import { SignedOut, api, h, languageName, seasonName } from "/web/lib.js";
import { evaluation } from "/web/profile-editor.js";

const SOURCES = {
  cam: "Cam",
  telesync: "Telesync",
  telecine: "Telecine",
  screener: "Screener",
  dvd: "DVD",
  hdtv: "HDTV",
  web_rip: "WEBRip",
  web_dl: "WEB-DL",
  bluray: "Blu-ray",
};

function episodes(e) {
  switch (e.kind) {
    case "standard":
      return `${seasonName(e.value.season)}, ${e.value.episodes.length > 1 ? "episodes" : "episode"} ${e.value.episodes.join(", ")}`;
    case "seasons":
      return `Whole ${e.value.length > 1 ? "seasons" : "season"} ${e.value.join(", ")}`;
    case "complete_series":
      return "Every season";
    case "absolute":
      return `${e.value.length > 1 ? "Episodes" : "Episode"} ${e.value.join(", ")}, numbered from the first`;
    case "daily":
      return `The episode of ${e.value}`;
    default:
      return null;
  }
}

const CODECS = { h264: "H.264", h265: "H.265", av1: "AV1", xvid: "Xvid", mpeg2: "MPEG-2" };

const words = (list) => list.filter(Boolean).join(", ");

// What the name says, a line for each part it gave.
function facts(p) {
  const hdr = [p.dolby_vision && "Dolby Vision", p.hdr10_plus && "HDR10+", p.hdr10 && "HDR10", p.hlg && "HLG"];
  const lines = [
    ["Title", p.year ? `${p.title} (${p.year})` : p.title],
    ["Episodes", episodes(p.episodes)],
    ["Quality", p.quality],
    [
      "Video",
      words([p.resolution && `${p.resolution}p`, SOURCES[p.source], p.remux && "Remux", p.disc && "Full disc", CODECS[p.video_codec], ...hdr]),
    ],
    ["Audio", words([p.audio_codec?.toUpperCase(), p.audio_channels, p.atmos && "Atmos", p.dual_audio && "Dual audio", p.multi_audio && "Several languages", ...p.languages.map(languageName)])],
    ["Subtitles", words([...p.subtitles.map(languageName), p.multi_subs && "Several languages", p.hardcoded_subs && "Burnt in"])],
    ["Release group", p.group],
    ["Repacked", p.proper ? `${p.proper} ${p.proper > 1 ? "times" : "time"} (PROPER, REPACK)` : null],
    ["Version", p.version ? `v${p.version}` : null],
  ];
  return h(
    "ul",
    { class: "glance" },
    lines.filter(([, value]) => value).map(([label, value]) => h("li", {}, h("span", { class: "fact-label" }, label), h("span", { class: "fact-value" }, value))),
  );
}

export async function releaseTry(ctx) {
  const profiles = await api("/profiles");
  if (!ctx.current()) return [];
  const name = field("try-release", "Release name", h("input", { autocomplete: "off", spellcheck: "false" }), "As an indexer lists it, e.g. Some.Film.2021.1080p.WEB-DL.DDP5.1.H.264-GROUP.");
  const forKind = (service, label) => {
    const mine = profiles.filter((p) => p.service === service);
    return mine.length ? h("optgroup", { label }, mine.map((p) => h("option", { value: p.id }, p.name))) : null;
  };
  const profile = field(
    "try-profile",
    "Quality profile (optional)",
    h("select", {}, h("option", { value: "" }, "None: only read the name"), forKind("movies", "Movies"), forKind("series", "Series"), forKind("anime", "Anime")),
    "Choose one to see whether it would take this release, and why.",
  );
  const existing = field("try-existing", "File already on disk (optional)", h("input", { autocomplete: "off", spellcheck: "false" }), "Its name, to see whether the release would replace it. Needs a profile.");
  const answer = h("div", { class: "try-answer", role: "status" });
  const button = h("button", { type: "submit" }, "Read the name");
  // An example to try with one click: it fills the name in and reads it.
  const example = h("button", { type: "button", class: "quiet" }, "Try an example");
  example.addEventListener("click", () => {
    name.input.value = EXAMPLE;
    formEl.requestSubmit();
  });
  const formEl = h("form", { novalidate: true }, name.box, profile.box, existing.box, h("div", { class: "form-actions" }, button, example));
  formEl.addEventListener("submit", async (event) => {
    event.preventDefault();
    name.clear();
    existing.clear();
    const release = name.input.value.trim();
    if (!release) {
      name.fail("Paste a release name.");
      name.input.focus();
      return;
    }
    const chosen = profile.input.value;
    const on = existing.input.value.trim();
    if (on && !chosen) {
      existing.fail("Choose a profile to compare it with, or leave this empty.");
      existing.input.focus();
      return;
    }
    button.disabled = true;
    answer.replaceChildren(h("p", { class: "note" }, "Reading it…"));
    try {
      const body = { name: release, ...(on ? { existing: on } : {}) };
      const [parsed, decided] = await Promise.all([
        api(`/releases/parse?${new URLSearchParams({ name: release })}`),
        chosen ? api(`/profiles/${chosen}/evaluate`, { method: "POST", body }) : null,
      ]);
      const profileName = profiles.find((p) => String(p.id) === chosen)?.name;
      answer.replaceChildren(
        h("h3", {}, "What the name says"),
        facts(parsed),
        ...(decided ? [h("h3", {}, `What ${profileName} would do`), ...evaluation(decided).filter(Boolean)] : []),
      );
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      answer.replaceChildren(h("p", { class: "error" }, err.message));
    } finally {
      button.disabled = false;
    }
  });
  // Opened from a release that was turned down: its name, and the profile that judged it.
  const asked = ctx.params?.get("name");
  if (asked) {
    name.input.value = asked;
    const judgedBy = ctx.params.get("profile");
    if (judgedBy && profiles.some((p) => String(p.id) === judgedBy)) profile.input.value = judgedBy;
    // Read once the page is drawn: a form not in the page can't be submitted.
    setTimeout(() => formEl.isConnected && formEl.requestSubmit());
  }
  return [
    h(
      "p",
      {},
      "Paste a release name from an indexer to see how Melete reads it: title, year, season and episode, quality, source, codec, audio, release group and the custom formats it matches. Choose a profile to see whether it would take the release, and why. Useful when a release you expected was turned down.",
    ),
    h("p", { class: "note" }, "Nothing is searched for or grabbed. To find releases for a title, search for it from its page or the wanted list."),
    formEl,
    answer,
  ];
}

// A link from a release that was turned down to this page, with its name and the profile that
// judged it filled in. None for music, which this page doesn't read.
export function tryLink(r, profileId) {
  if (r.source === "soulseek" || !profileId) return null;
  const params = new URLSearchParams({ name: r.title, profile: String(profileId) });
  return h("a", { href: `#/admin/indexers/try?${params}`, "aria-label": `Try ${r.title} in Try a release name` }, "Try this name");
}

// A release name as indexers list them, to try the page with.
const EXAMPLE = "Some.Film.2021.2160p.UHD.BluRay.REMUX.DV.HDR.HEVC.TrueHD.Atmos.7.1-GROUP";
