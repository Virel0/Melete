// Editing quality profiles and custom formats, and trying a release name against a
// profile, through the same API as everything else. Admins only. A profile edited here becomes
// Melete's own, which re-imports from Radarr or Sonarr leave alone.
// A custom format's conditions are edited in condition-editor.js; what both editors share is in
// editor-parts.js.

import { conditionEditor } from "/web/condition-editor.js";
import { actions, check, sending, uid } from "/web/editor-parts.js";
import { field } from "/web/forms.js";
import { SignedOut, announce, api, h, plural } from "/web/lib.js";

// Every quality as Melete names them (src/profiles/quality.rs QUALITIES), lowest first.
export const QUALITIES = [
  "Unknown", "WORKPRINT", "CAM", "TELESYNC", "TELECINE", "REGIONAL", "DVDSCR", "SDTV", "DVD", "DVD-R",
  "WEBDL-480p", "WEBRip-480p", "Bluray-480p", "Bluray-576p", "HDTV-720p", "WEBDL-720p", "WEBRip-720p",
  "Bluray-720p", "HDTV-1080p", "WEBDL-1080p", "WEBRip-1080p", "Bluray-1080p", "Remux-1080p", "HDTV-2160p",
  "WEBDL-2160p", "WEBRip-2160p", "Bluray-2160p", "Remux-2160p", "BR-DISK", "Raw-HD",
];

export const FOR = { radarr: "movies", sonarr: "series" };

// A whole number, or null when the box doesn't hold one.
function whole(input) {
  const text = input.value.trim();
  return /^-?\d+$/.test(text) ? Number(text) : null;
}


// ---------- Profiles ----------

// A profile to start a new one from: every quality, none allowed yet.
export function blankProfile(source) {
  return {
    source,
    name: "",
    upgrade_allowed: true,
    cutoff: "",
    min_format_score: 0,
    cutoff_format_score: 0,
    min_upgrade_format_score: 1,
    language: source === "radarr" ? "original" : null,
    items: QUALITIES.map((q) => ({ name: q, qualities: [q], allowed: false })),
    formats: [],
  };
}

// The form for a profile: `profile.id` set to change it, unset to make one. `formats` are the
// custom formats there are; `done(message)` goes back to the list, null for unchanged.
export function profileEditor({ profile, formats, done, ctx }) {
  const editing = profile.id !== undefined;
  const name = field(uid("profile-name"), "Name", h("input", { value: profile.name, autocomplete: "off" }), "Unique among the profiles for " + FOR[profile.source] + ".");
  const upgrades = check(uid("profile-upgrades"), "Upgrade files until they reach the cutoff", profile.upgrade_allowed);
  const q = qualityEditor(profile);
  const scores = scoreFields(profile);
  const { minScore, cutoffScore, stepScore, language } = scores;
  const { ofKind, scoreInputs } = formatScores(profile, formats);

  const foot = actions(editing ? "Save profile" : "Make profile", () => done(null));
  const formEl = h(
    "form",
    { novalidate: true, class: "editor" },
    h("h2", {}, editing ? `Edit ${profile.name}` : `New profile for ${FOR[profile.source]}`),
    editing && profile.origin === "imported" ? h("p", { class: "note" }, "Once saved here it's Melete's own: importing from " + (profile.source === "radarr" ? "Radarr" : "Sonarr") + " again leaves it alone.") : null,
    name.box,
    h("h3", {}, "Qualities"),
    h("p", { class: "note" }, "Lowest first. Tick the ones to take; a higher one replaces a lower one."),
    q.list,
    h("div", { class: "field" }, h("label", { for: q.cutoff.id }, "Cutoff"), h("p", { class: "note" }, "Upgrades stop once a file is this good."), q.cutoff, q.cutoffError),
    upgrades.el,
    h("h3", {}, "Custom format scores"),
    formatsAbout(profile.source, ofKind.length),
    ofKind.length ? h("ul", { class: "rows scores" }, scoreInputs.map((s) => s.el)) : null,
    // Without formats every release scores 0, so the score boxes would do nothing; they keep
    // their values, hidden, until there are formats.
    h("div", { hidden: !ofKind.length }, minScore.box, cutoffScore.box, stepScore.box),
    language?.box,
    foot.el,
  );

  sending(
    formEl,
    foot,
    async () => {
      const body = readProfile(profile, { name, upgrades, q, scores, scoreInputs, foot });
      if (!body) return null;
      const saved = editing
        ? await api(`/profiles/${profile.id}`, { method: "PUT", body })
        : await api("/profiles", { method: "POST", body });
      return editing ? `Saved ${saved.name}.` : `Made ${saved.name}.`;
    },
    done,
    ctx,
  );
  requestAnimationFrame(() => name.input.focus());
  return formEl;
}

// The profile's qualities, lowest first, to tick, reorder, join and split, and the cutoff
// picked from the ticked ones. Returns their shared state: the rows, the list and the cutoff.
function qualityEditor(profile) {
  const q = {
    profile,
    items: profile.items.map((i) => ({ ...i })),
    cutoff: h("select", { id: uid("profile-cutoff") }),
    cutoffError: h("p", { class: "field-error", hidden: true }),
    // The cutoff to keep across a rename, join or split, which change its row's name.
    wantedCutoff: null,
    list: h("ol", { class: "rows qualities" }),
  };
  drawItems(q);
  drawCutoff(q);
  return q;
}

// The cutoff's choices: the ticked qualities, keeping the one chosen.
function drawCutoff(q) {
  const was = q.wantedCutoff ?? (q.cutoff.value || q.profile.cutoff);
  q.wantedCutoff = null;
  const allowed = q.items.filter((i) => i.allowed);
  q.cutoff.replaceChildren(...allowed.map((i) => h("option", { value: i.name, selected: i.name === was }, i.name)));
  if (!allowed.length) q.cutoff.append(h("option", { value: "" }, "Allow a quality first"));
}

// Draws every quality row; `focus` says which row and control to focus afterwards.
function drawItems(q, focus) {
  q.list.replaceChildren(...q.items.map((item, index) => qualityRow(q, item, index, focus)));
}

// One quality (or group): its tick, Lower and Higher, Join with previous, Split, and a
// group's name.
function qualityRow(q, item, index, focus) {
  const { items } = q;
  const id = uid("quality");
  const allowed = h("input", { type: "checkbox", id, checked: item.allowed });
  allowed.addEventListener("change", () => {
    item.allowed = allowed.checked;
    drawCutoff(q);
  });
  const move = (by) => {
    const to = index + by;
    [items[index], items[to]] = [items[to], items[index]];
    drawItems(q, { name: item.name, by });
  };
  const up = h("button", { type: "button", class: "quiet", disabled: index === 0, "aria-label": `Move ${item.name} lower` }, "Lower");
  const down = h("button", { type: "button", class: "quiet", disabled: index === items.length - 1, "aria-label": `Move ${item.name} higher` }, "Higher");
  up.addEventListener("click", () => move(-1));
  down.addEventListener("click", () => move(1));
  const group = item.qualities.length > 1 || item.qualities[0] !== item.name;
  // Joining makes a group with the row before it, ranked as one; splitting undoes that.
  const join = index > 0 ? h("button", { type: "button", class: "quiet", "aria-label": `Join ${item.name} with ${items[index - 1].name} as one group` }, "Join with previous") : null;
  join?.addEventListener("click", () => joinWithPrevious(q, index));
  const split = group ? h("button", { type: "button", class: "quiet", "aria-label": `Split ${item.name} into its qualities` }, "Split") : null;
  split?.addEventListener("click", () => splitGroup(q, index));
  let groupName = null;
  if (group) {
    const nameId = uid("group-name");
    const input = h("input", { id: nameId, value: item.name, autocomplete: "off", maxlength: "64" });
    input.addEventListener("change", () => {
      const next = input.value.trim();
      if (!next || items.some((other) => other !== item && other.name === next)) {
        input.value = item.name;
        return;
      }
      if (q.cutoff.value === item.name) q.wantedCutoff = next;
      item.name = next;
      drawItems(q, { name: next, field: "name" });
      drawCutoff(q);
    });
    groupName = h("div", { class: "field group-name" }, h("label", { for: nameId }, "Group name"), input);
  }
  const li = h(
    "li",
    {},
    h(
      "div",
      {},
      h("div", { class: "check" }, allowed, h("label", { for: id }, item.name)),
      group ? h("p", { class: "note" }, `A group, ranked as one: ${item.qualities.join(", ")}.`) : null,
      groupName,
    ),
    h("div", { class: "row-actions" }, up, down, join, split),
  );
  if (focus?.name === item.name) {
    requestAnimationFrame(() => {
      if (focus.field === "name") return groupName?.querySelector("input")?.focus();
      if (focus.field === "check") return allowed.focus();
      const button = focus.by < 0 ? up : down;
      (button.disabled ? (focus.by < 0 ? down : up) : button).focus();
    });
  }
  return li;
}

// A name not taken by another row: "A / B", then "A / B 2" and so on.
function freeName(items, wanted) {
  let name = wanted;
  for (let n = 2; items.some((i) => i.name === name); n++) name = `${wanted} ${n}`;
  return name;
}

// Joins a row with the one before it as a group, ranked as one.
function joinWithPrevious(q, index) {
  const { items } = q;
  const [lower, higher] = [items[index - 1], items[index]];
  const joined = {
    name: "",
    qualities: [...lower.qualities, ...higher.qualities],
    allowed: lower.allowed || higher.allowed,
  };
  items.splice(index - 1, 2, joined);
  joined.name = freeName(items, `${lower.name} / ${higher.name}`);
  if ([lower.name, higher.name].includes(q.cutoff.value)) q.wantedCutoff = joined.name;
  drawItems(q, { name: joined.name, field: "name" });
  drawCutoff(q);
  announce(`Joined ${lower.name} and ${higher.name} as ${joined.name}`);
}

// Splits a group back into its qualities, each ticked as the group was.
function splitGroup(q, index) {
  const { items } = q;
  const grouped = items[index];
  const singles = grouped.qualities.map((name) => ({ name, qualities: [name], allowed: grouped.allowed }));
  items.splice(index, 1, ...singles);
  // A group that was the cutoff hands it to its best quality.
  if (q.cutoff.value === grouped.name) q.wantedCutoff = singles[singles.length - 1].name;
  drawItems(q, { name: singles[0].name, field: "check" });
  drawCutoff(q);
  announce(`Split ${grouped.name} into ${plural(singles.length, "quality", "qualities")}`);
}

// The three format-score boxes, and the audio language for a movie profile.
function scoreFields(profile) {
  const number = (label, value, hint) => field(uid("profile-score"), label, h("input", { value: String(value), inputmode: "numeric", class: "year", autocomplete: "off" }), hint);
  const minScore = number("Lowest format score taken", profile.min_format_score, "Releases whose custom formats add up to less are turned down. For example, 0 turns down releases scoring below 0.");
  const cutoffScore = number("Upgrade until this format score", profile.cutoff_format_score, "A file at the cutoff quality scoring this much isn't upgraded. For example, 100 stops upgrading once a file at the cutoff quality scores 100.");
  const stepScore = number("Score an upgrade must gain", profile.min_upgrade_format_score, "At the same quality, how much more a release must score to replace a file. For example, 10 only replaces a file with a release scoring at least 10 more. At least 1.");
  const language =
    profile.source === "radarr"
      ? field(uid("profile-language"), "Audio language", h("input", { value: profile.language ?? "", autocomplete: "off", class: "year" }), "A two-letter code (en, ja…), original for the film's own, or any.")
      : null;
  return { minScore, cutoffScore, stepScore, language };
}

// What custom formats are and where they're made: the whole section while there are none, a
// line over the scores once there are. The link stays in the service the page is in.
function formatsAbout(source, count) {
  const here = location.hash.match(/^#\/admin\/(movies|series|anime)\//)?.[1] ?? (source === "radarr" ? "movies" : "series");
  const app = source === "radarr" ? "Radarr" : "Sonarr";
  const what = "Custom formats are rules that match parts of a release name, such as x265, HDR, Dolby Vision or a release group, and give each match a score. A release scores the sum of the formats it matches.";
  if (count) return h("p", { class: "note" }, `${what} Each format's score for this profile:`);
  return h(
    "div",
    {},
    h("p", { class: "note" }, `${what} There are none for ${FOR[source]} yet, so every release scores 0; the score settings appear once there are some.`),
    h("p", { class: "note" }, h("a", { href: `#/admin/${here}/formats` }, "Make custom formats"), `, or import ${app}'s with its profiles from the Profiles page.`),
  );
}

// A score box for each custom format of the profile's kind.
function formatScores(profile, formats) {
  const scores = new Map(profile.formats.map((f) => [f.format_id, f.score]));
  const ofKind = formats.filter((f) => f.source === profile.source);
  const scoreInputs = ofKind.map((f) => {
    const id = uid("format-score");
    const input = h("input", { id, value: String(scores.get(f.id) ?? 0), inputmode: "numeric", class: "year", autocomplete: "off" });
    return { format: f, input, el: h("li", {}, h("div", {}, h("label", { for: id }, f.name)), input) };
  });
  return { ofKind, scoreInputs };
}

// The profile as the form has it, for the API; null after marking what needs fixing.
function readProfile(profile, { name, upgrades, q, scores, scoreInputs, foot }) {
  const { minScore, cutoffScore, stepScore, language } = scores;
  const body = {
    source: profile.source,
    name: name.input.value.trim(),
    upgrade_allowed: upgrades.box.checked,
    cutoff: q.cutoff.value,
    min_format_score: whole(minScore.input),
    cutoff_format_score: whole(cutoffScore.input),
    min_upgrade_format_score: whole(stepScore.input),
    language: language ? language.input.value.trim() || null : null,
    items: q.items.map(({ name, qualities, allowed }) => ({ name, qualities, allowed })),
    formats: [],
  };
  let wrong = false;
  if (!body.name) {
    name.fail("Give the profile a name.");
    wrong = true;
  }
  if (!body.cutoff) {
    q.cutoffError.textContent = "Allow at least one quality, and choose where upgrades stop.";
    q.cutoffError.hidden = false;
    q.cutoff.setAttribute("aria-invalid", "true");
    wrong = true;
  }
  for (const [box, key] of [[minScore, "min_format_score"], [cutoffScore, "cutoff_format_score"], [stepScore, "min_upgrade_format_score"]]) {
    if (body[key] === null) {
      box.fail("Enter a whole number.");
      wrong = true;
    }
  }
  if (body.min_upgrade_format_score !== null && body.min_upgrade_format_score < 1) {
    stepScore.fail("At least 1.");
    wrong = true;
  }
  for (const s of scoreInputs) {
    const score = whole(s.input);
    if (score === null) {
      s.input.setAttribute("aria-invalid", "true");
      wrong = true;
    } else if (score !== 0) {
      body.formats.push({ format_id: s.format.id, score });
    }
  }
  if (wrong) {
    foot.result.textContent = "Some of it needs fixing first.";
    foot.result.classList.add("error");
    return null;
  }
  return body;
}

// What a profile would do with a release name: taken or not, and why.
export function tryRelease(profile, ctx) {
  const release = field(uid("try-name"), "Release name", h("input", { autocomplete: "off", spellcheck: "false" }), "As an indexer lists it, e.g. Some.Film.2021.1080p.WEB-DL.DDP5.1.H.264-GROUP.");
  const existing = field(uid("try-existing"), "File already on disk (optional)", h("input", { autocomplete: "off", spellcheck: "false" }), "Its name, to see whether this would replace it.");
  const answer = h("div", { role: "status" });
  const button = h("button", { type: "submit" }, "Try it");
  const formEl = h("form", { novalidate: true, class: "try" }, release.box, existing.box, h("div", { class: "form-actions" }, button), answer);
  formEl.addEventListener("submit", async (event) => {
    event.preventDefault();
    release.clear();
    const name = release.input.value.trim();
    if (!name) {
      release.fail("Paste a release name.");
      release.input.focus();
      return;
    }
    const body = { name };
    const on = existing.input.value.trim();
    if (on) body.existing = on;
    button.disabled = true;
    try {
      const said = await api(`/profiles/${profile.id}/evaluate`, { method: "POST", body });
      answer.replaceChildren(...evaluation(said).filter(Boolean));
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      answer.replaceChildren(h("p", { class: "error" }, err.message));
    } finally {
      button.disabled = false;
    }
  });
  return formEl;
}

// The answer, in words; Indexers → Try a release name shows it too.
export function evaluation({ decision, existing }) {
  const formats = (list) => (list.length ? list.map((f) => `${f.name} ${f.score > 0 ? "+" : ""}${f.score}`).join(", ") : "none");
  return [
    h("p", {}, h("strong", {}, decision.accepted ? "Taken." : "Turned down."), ` ${decision.quality}, scoring ${decision.score}.`),
    decision.rejections.length ? h("ul", { class: "plain" }, decision.rejections.map((r) => h("li", { class: "error" }, r))) : null,
    h("p", { class: "note" }, `Formats: ${formats(decision.formats)}. Audio: ${decision.languages.join(", ") || "unknown"}.`),
    existing ? h("p", { class: "note" }, `The file on disk: ${existing.quality}, scoring ${existing.score} (formats: ${formats(existing.formats)}).`) : null,
  ];
}

// ---------- Custom formats ----------

// The form for a custom format: `format.id` set to change it, unset to make one.
export function formatEditor({ format, done, ctx }) {
  const editing = format.id !== undefined;
  const name = field(uid("format-name"), "Name", h("input", { value: format.name, autocomplete: "off" }), `Unique among the formats for ${FOR[format.source]}.`);
  const box = h("div", { class: "conditions" });
  let editors = [];
  const relabel = () => editors.forEach((e, i) => (e.legend.textContent = `Condition ${i + 1}`));
  const add = (condition) => {
    const editor = conditionEditor(condition, () => {
      editors = editors.filter((e) => e !== editor);
      editor.el.remove();
      relabel();
      addButton.focus();
    });
    editors.push(editor);
    box.append(editor.el);
    relabel();
    return editor;
  };
  const addButton = h("button", { type: "button", class: "quiet" }, "Add a condition");
  addButton.addEventListener("click", () => {
    const editor = add({ name: "", kind: "release_title", negate: false, required: false });
    editor.el.querySelector("input")?.focus();
  });
  (format.conditions.length ? format.conditions : [{ name: "", kind: "release_title", negate: false, required: false }]).forEach(add);

  const foot = actions(editing ? "Save format" : "Make format", () => done(null));
  const formEl = h(
    "form",
    { novalidate: true, class: "editor" },
    h("h2", {}, editing ? `Edit ${format.name}` : `New custom format for ${FOR[format.source]}`),
    h("p", { class: "note" }, "Conditions of one kind match when every required one holds and at least one holds; the format matches when every kind does."),
    name.box,
    box,
    h("div", { class: "form-actions" }, addButton),
    foot.el,
  );
  sending(
    formEl,
    foot,
    async () => {
      const body = { source: format.source, name: name.input.value.trim(), conditions: [] };
      let wrong = false;
      if (!body.name) {
        name.fail("Give the format a name.");
        wrong = true;
      }
      for (const e of editors) {
        const read = e.read();
        if (read) body.conditions.push(read);
        else wrong = true;
      }
      if (!editors.length) {
        foot.result.textContent = "A format needs at least one condition.";
        foot.result.classList.add("error");
        return null;
      }
      if (wrong) {
        foot.result.textContent = "Some of it needs fixing first.";
        foot.result.classList.add("error");
        return null;
      }
      const saved = editing
        ? await api(`/custom-formats/${format.id}`, { method: "PUT", body })
        : await api("/custom-formats", { method: "POST", body });
      return editing ? `Saved ${saved.name}.` : `Made ${saved.name}.`;
    },
    done,
    ctx,
  );
  requestAnimationFrame(() => name.input.focus());
  return formEl;
}
