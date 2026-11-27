// One setting of an outside service or a feature, as its page draws it: a checkbox, a list, a
// secret (only ever said to be set, with Remove it) or a box to type in, and what of it is to
// be saved. A setting given in .env is shown, not changed, naming its variable and the way to
// unlock it.

import { field } from "/web/forms.js";
import { h } from "/web/lib.js";

// Why a setting given in .env can't be changed here: its variable, and the way back.
const locked = (f) => ["Locked by ", h("code", {}, f.name), " in .env. Remove that line and restart Melete to change it here."];

// One field's input, and how to read what's to be saved from it: a value, null to clear, or
// undefined to leave it as it is.
export function input(service, f) {
  const id = `service-${f.name.toLowerCase()}`;
  const fixed = f.from === "environment";
  if (f.input.toggle) {
    const note = fixed ? locked(f) : f.hint || null;
    const box = h("input", { type: "checkbox", id, checked: f.value === f.input.toggle.on, disabled: fixed, "aria-describedby": note ? `${id}-hint` : null });
    const el = h("div", { class: "check" }, box, h("label", { for: id }, f.label));
    return {
      box: h("div", { class: "field" }, el, note ? h("p", { class: "note", id: `${id}-hint` }, note) : null),
      input: box,
      clear() {},
      fail() {},
      read: () => (fixed ? undefined : box.checked ? f.input.toggle.on : "off"),
    };
  }
  if (f.input.choice) {
    const { options } = f.input.choice;
    const current = f.value ?? options[0]?.value;
    const select = h("select", { disabled: fixed }, options.map((o) => h("option", { value: o.value, selected: o.value === current }, o.label)));
    const made = field(id, f.label, select, fixed ? locked(f) : f.hint || null);
    // Only a change is sent, so saving the service's other settings doesn't judge every title again.
    return { ...made, read: () => (fixed || select.value === current ? undefined : select.value) };
  }
  if (f.input === "secret") {
    const hint = fixed ? locked(f) : f.set ? `Set. Leave it empty to keep it.${f.hint ? ` ${f.hint}` : ""}` : f.hint || "Not set.";
    const made = field(id, f.label, h("input", { type: "password", autocomplete: "new-password", spellcheck: "false", disabled: fixed }), hint);
    const removeId = `${id}-remove`;
    const remove = h("input", { type: "checkbox", id: removeId });
    if (f.set && f.from === "settings") {
      made.box.append(h("div", { class: "check" }, remove, h("label", { for: removeId }, "Remove it")));
    }
    return {
      ...made,
      read: () => (fixed ? undefined : remove.checked ? null : made.input.value.trim() || undefined),
    };
  }
  const made = field(
    id,
    f.label,
    h("input", {
      value: f.value ?? "",
      inputmode: f.input === "number" ? "numeric" : null,
      autocomplete: "off",
      spellcheck: "false",
      disabled: fixed,
      class: f.input === "number" ? "year" : null,
    }),
    fixed ? locked(f) : f.hint || null,
  );
  return { ...made, read: () => (fixed ? undefined : made.input.value.trim() || null) };
}

// The values to send: only what's changed or to be cleared.
export function values(inputs) {
  const out = {};
  for (const [name, i] of inputs) {
    const value = i.read();
    if (value !== undefined) out[name] = value;
  }
  return out;
}
