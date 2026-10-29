// A custom format's conditions: the kinds there are, one condition's editor, and a format's
// conditions in a line for the lists.

import { check, uid } from "/web/editor-parts.js";
import { field } from "/web/forms.js";
import { h, plural } from "/web/lib.js";

// What each kind of condition tests, and the fields it needs.
const KINDS = {
  release_title: { label: "Release name matches", fields: ["pattern"] },
  release_group: { label: "Release group matches", fields: ["pattern"] },
  edition: { label: "Edition matches", fields: ["pattern"] },
  language: { label: "Audio language", fields: ["language", "except"] },
  subtitles: { label: "Subtitles in", fields: ["language"] },
  source: { label: "Source", fields: ["sources"] },
  resolution: { label: "Resolution", fields: ["resolution"] },
  modifier: { label: "Modifier", fields: ["modifier"] },
  size: { label: "Size", fields: ["min", "max"] },
  year: { label: "Year", fields: ["min", "max"] },
  release_type: { label: "Release type", fields: ["release_type"] },
  indexer_flag: { label: "Indexer flag", fields: ["flag"] },
};
const SOURCES = ["unknown", "workprint", "cam", "telesync", "telecine", "dvd", "tv", "tv_raw", "web_dl", "web_rip", "bluray", "bluray_raw"];
const MODIFIERS = ["none", "regional", "screener", "raw_hd", "br_disk", "remux"];
const RELEASE_TYPES = ["unknown", "single_episode", "multi_episode", "season_pack"];
const RESOLUTIONS = [360, 480, 540, 576, 720, 1080, 2160];
const words = (value) => value.replaceAll("_", " ");

// One condition's editor; `read()` gives it as the API takes it, or null after marking what's wrong.
export function conditionEditor(condition, onRemove) {
  const c = { ...condition };
  const fieldset = h("fieldset", { class: "condition" });
  const legend = h("legend", {});
  const name = field(uid("cond-name"), "Name", h("input", { value: c.name ?? "", autocomplete: "off" }), "For you: shown in decisions.");
  const kind = h("select", { id: uid("cond-kind") }, Object.entries(KINDS).map(([k, v]) => h("option", { value: k, selected: k === c.kind }, v.label)));
  const values = h("div");
  const negate = check(uid("cond-negate"), "Holds when the test fails (not)", c.negate);
  const required = check(uid("cond-required"), "Required: must hold for its kind to match", c.required);
  const remove = h("button", { type: "button", class: "quiet" }, "Remove condition");
  remove.addEventListener("click", onRemove);
  let inputs = {};

  const drawValues = () => {
    inputs = {};
    const parts = [];
    for (const f of KINDS[kind.value].fields) {
      const part = valueField(f, uid(`cond-${f}`), kind.value, c, inputs);
      if (part) parts.push(part);
    }
    values.replaceChildren(...parts);
  };
  kind.addEventListener("change", drawValues);
  drawValues();

  fieldset.append(legend, name.box, h("div", { class: "field" }, h("label", { for: kind.id }, "Tests"), kind), values, negate.el, required.el, h("div", { class: "row-actions" }, remove));
  return {
    el: fieldset,
    legend,
    read() {
      return readCondition({ name, negate, required, kind, inputs });
    },
  };
}

// The box (or boxes) for one of a kind's fields, `f`, with its parts recorded in `inputs` for
// reading back; null for a field there's no box for.
function valueField(f, id, kindName, c, inputs) {
  if (f === "pattern") {
    const made = field(id, "Pattern", h("input", { value: c.pattern ?? "", autocomplete: "off", spellcheck: "false" }), "A regular expression, matched ignoring case; lookarounds work.");
    inputs.pattern = made;
    return made.box;
  } else if (f === "language") {
    const made = field(id, "Language", h("input", { value: c.language ?? "", autocomplete: "off", class: "year" }), kindName === "subtitles" ? "A two-letter code, or any." : "A two-letter code, original, unknown or any.");
    inputs.language = made;
    return made.box;
  } else if (f === "except") {
    const made = check(id, "Any language but this one", c.except ?? false);
    inputs.except = made;
    return made.el;
  } else if (f === "sources") {
    const chosen = new Set(c.sources ?? []);
    const boxes = SOURCES.map((s) => ({ s, ...check(uid("cond-source"), words(s), chosen.has(s)) }));
    const error = h("p", { class: "field-error", hidden: true }, "Tick at least one source.");
    inputs.sources = boxes;
    inputs.sourcesError = error;
    return h("fieldset", { class: "choices" }, h("legend", {}, "Any of these sources"), boxes.map((b) => b.el), error);
  } else if (f === "resolution") {
    const select = h("select", { id }, RESOLUTIONS.map((r) => h("option", { value: r, selected: r === c.resolution }, `${r}p`)));
    inputs.resolution = select;
    return h("div", { class: "field" }, h("label", { for: id }, "Resolution"), select);
  } else if (f === "modifier" || f === "release_type") {
    const options = f === "modifier" ? MODIFIERS : RELEASE_TYPES;
    const select = h("select", { id }, options.map((o) => h("option", { value: o, selected: o === c[f] }, words(o))));
    inputs[f] = select;
    return h("div", { class: "field" }, h("label", { for: id }, f === "modifier" ? "Modifier" : "Release type"), select);
  } else if (f === "min" || f === "max") {
    const size = kindName === "size";
    const label = f === "min" ? (size ? "More than (GiB)" : "From") : size ? "At most (GiB)" : "To";
    const made = field(id, label, h("input", { value: c[f] ?? "", inputmode: size ? "decimal" : "numeric", class: "year", autocomplete: "off" }));
    inputs[f] = made;
    return made.box;
  } else if (f === "flag") {
    const made = field(id, "Flag", h("input", { value: c.flag ?? "", autocomplete: "off", class: "year" }), "freeleech, halfleech, double_upload, internal, scene, nuked…");
    inputs.flag = made;
    return made.box;
  }
  return null;
}

// The condition as the API takes it, or null after marking what's wrong.
function readCondition({ name, negate, required, kind, inputs }) {
  const out = { name: name.input.value.trim(), negate: negate.box.checked, required: required.box.checked, kind: kind.value };
  let ok = true;
  if (!out.name) {
    name.fail("Give the condition a name.");
    ok = false;
  }
  const text = (made, key, message) => {
    const value = made.input.value.trim();
    if (!value) {
      made.fail(message);
      ok = false;
    }
    out[key] = value;
  };
  const num = (made, key, decimal) => {
    const value = made.input.value.trim();
    const parsed = decimal ? Number(value) : /^-?\d+$/.test(value) ? Number(value) : NaN;
    if (!value || Number.isNaN(parsed)) {
      made.fail(decimal ? "Enter a number." : "Enter a whole number.");
      ok = false;
    }
    out[key] = parsed;
  };
  if (inputs.pattern) text(inputs.pattern, "pattern", "Enter a pattern.");
  if (inputs.language) text(inputs.language, "language", "Enter a language.");
  if (inputs.except) out.except = inputs.except.box.checked;
  if (inputs.flag) text(inputs.flag, "flag", "Enter a flag.");
  if (inputs.sources) {
    out.sources = inputs.sources.filter((b) => b.box.checked).map((b) => b.s);
    if (!out.sources.length) {
      inputs.sources[0].box.setAttribute("aria-invalid", "true");
      inputs.sourcesError.hidden = false;
      ok = false;
    }
  }
  if (inputs.resolution) out.resolution = Number(inputs.resolution.value);
  if (inputs.modifier) out.modifier = inputs.modifier.value;
  if (inputs.release_type) out.release_type = inputs.release_type.value;
  if (inputs.min) num(inputs.min, "min", kind.value === "size");
  if (inputs.max) num(inputs.max, "max", kind.value === "size");
  return ok ? out : null;
}

// A format's conditions in a line.
export function conditionSummary(format) {
  return `${plural(format.conditions.length, "condition", "conditions")}: ${format.conditions.map((c) => `${c.negate ? "not " : ""}${KINDS[c.kind]?.label.toLowerCase() ?? c.kind}`).join(", ")}.`;
}
