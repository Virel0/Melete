// How the web UI looks in this browser: Light, Dark, or System (the device's choice). Kept by
// this browser alone, and applied before the page is drawn (theme.js); a part of Your account.

import { group } from "/web/forms.js";
import { announce, h } from "/web/lib.js";

const KEY = "melete.appearance";
const CHOICES = [
  ["system", "System", "As this device is set, changing when it does."],
  ["light", "Light", null],
  ["dark", "Dark", null],
];

// What this browser keeps; System when it keeps nothing, or storage is turned off.
function chosen() {
  try {
    const kept = localStorage.getItem(KEY);
    return CHOICES.some(([value]) => value === kept) ? kept : "system";
  } catch {
    return "system";
  }
}

// Keeps `value` and applies it at once; false when this browser's storage is turned off.
function choose(value) {
  try {
    if (value === "system") localStorage.removeItem(KEY);
    else localStorage.setItem(KEY, value);
  } catch {
    return false;
  }
  dispatchEvent(new Event("melete:appearance"));
  return true;
}

export function appearanceGroup() {
  const now = chosen();
  const said = h("p", { class: "form-result", role: "status" });
  const options = CHOICES.map(([value, label, hint]) => {
    const id = `appearance-${value}`;
    const input = h("input", { type: "radio", name: "appearance", id, value, checked: value === now });
    input.addEventListener("change", () => {
      if (!input.checked) return;
      if (choose(value)) announce(`Appearance: ${label}`);
      else said.textContent = "This browser isn't keeping settings (a private window, say), so it can't be changed here.";
    });
    const row = h("div", { class: "check" }, input, h("label", { for: id }, label));
    if (!hint) return row;
    input.setAttribute("aria-describedby", `${id}-hint`);
    return [row, h("p", { class: "note check-hint", id: `${id}-hint` }, hint)];
  });
  return group(
    "Appearance",
    h("p", { class: "note" }, "Kept by this browser only."),
    h("fieldset", { class: "appearance" }, h("legend", { class: "visually-hidden" }, "Appearance"), options),
    said,
  );
}
