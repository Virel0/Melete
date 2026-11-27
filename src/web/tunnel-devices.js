// My devices: your household devices on Melete's own WireGuard tunnel, on your account page.
// Each reaches sign-in and the web page, plus the parts it's given, as you alone. Adding one
// shows its keys once; removing one cuts it off at once.

import { confirmInline, field, form, group, when } from "/web/forms.js";
import { announce, api, h } from "/web/lib.js";

const PART_NAMES = { video: "Video", music: "Music", requests: "Requests", admin: "Admin" };

// What the form offers, from widest to the parts picked one by one.
const CHOICES = [
  { value: "full", label: "Full: everything you can reach", parts: ["video", "music", "requests", "admin"] },
  { value: "video", label: "Video app: video and requests", parts: ["video", "requests"] },
  { value: "music", label: "Music app: music and requests", parts: ["music", "requests"] },
  { value: "custom", label: "Choose the parts", parts: null },
];

const partsText = (parts) => parts.map((p) => PART_NAMES[p] ?? p).join(", ");

// The section, or nothing when the tunnel is off and there are no devices to show.
export async function devicesGroup(ctx) {
  const listed = await api("/tunnel/devices");
  if (!listed.enabled && listed.devices.length === 0) return null;
  const list = h("ul", { class: "rows" });
  const empty = h("p", { class: "status" }, "No devices yet.");
  const draw = (devices) => {
    list.replaceChildren(...devices.map((d) => deviceRow(d, list, refresh)));
    list.hidden = devices.length === 0;
    empty.hidden = devices.length > 0;
  };
  const refresh = async () => draw((await api("/tunnel/devices")).devices);
  draw(listed.devices);
  const adding = listed.enabled
    ? addForm(ctx, refresh, listed.limit)
    : [h("p", { class: "note" }, "Melete's own tunnel is off on this server, so no device can be added.")];
  return group(
    "My devices",
    h(
      "p",
      { class: "note" },
      "Devices that reach Melete through its own WireGuard tunnel when they're away from home. Each one reaches signing in and this page, plus the parts you give it, as you alone.",
    ),
    list,
    empty,
    ...adding,
  );
}

// A device, with Remove.
function deviceRow(d, list, refresh) {
  const actions = h("div", { class: "row-actions" });
  const remove = h("button", { type: "button", class: "quiet" }, "Remove");
  remove.addEventListener("click", () =>
    confirmInline(actions, {
      question: `Remove “${d.name}”? It's cut off from Melete at once, and its config stops working.`,
      confirm: "Remove device",
      run: async () => {
        await api(`/tunnel/devices/${d.id}`, { method: "DELETE" });
        announce(`Removed ${d.name}`);
        await refresh();
        list.querySelector("button")?.focus();
      },
    }),
  );
  actions.append(remove);
  return h(
    "li",
    {},
    h(
      "div",
      {},
      h("strong", {}, d.name),
      h("p", { class: "note" }, `${partsText(d.parts)}. Address ${d.address ?? "unknown"}; added ${when(d.created_at)}.`),
    ),
    actions,
  );
}

// The parts' checkboxes, for "Choose the parts".
function partBoxes(isAdmin) {
  const names = Object.keys(PART_NAMES).filter((p) => p !== "admin" || isAdmin);
  const boxes = names.map((p) => {
    const box = h("input", { type: "checkbox", id: `device-part-${p}`, value: p });
    return { box, el: h("div", { class: "check" }, box, h("label", { for: box.id }, PART_NAMES[p])) };
  });
  const error = h("p", { class: "field-error", id: "device-parts-error", hidden: true });
  const fieldset = h(
    "fieldset",
    { class: "choices", "aria-describedby": error.id, hidden: true },
    h("legend", {}, "Parts it reaches"),
    boxes.map((b) => b.el),
    error,
  );
  return {
    box: fieldset,
    picked: () => boxes.filter((b) => b.box.checked).map((b) => b.box.value),
    fail(message) {
      error.textContent = message;
      error.hidden = false;
      boxes[0]?.box.setAttribute("aria-invalid", "true");
    },
    clear() {
      error.hidden = true;
      boxes[0]?.box.removeAttribute("aria-invalid");
    },
  };
}

// Adding a device; returns the form and where its keys appear, once.
function addForm(ctx, refresh, limit) {
  const isAdmin = Boolean(ctx.me?.is_admin);
  const choices = CHOICES.map((c) => (c.value === "full" && !isAdmin ? { ...c, parts: ["video", "music", "requests"] } : c));
  const name = field("device-name", "Name", h("input", { autocomplete: "off", maxlength: "60" }), "Which device it is, e.g. “Living room iPad”.");
  const kind = field(
    "device-kind",
    "What it reaches",
    h("select", {}, choices.map((c) => h("option", { value: c.value }, c.label))),
    "Signing in and this page are always reachable. Give it no more than the app on it needs.",
  );
  const parts = partBoxes(isAdmin);
  kind.input.addEventListener("change", () => {
    parts.box.hidden = kind.input.value !== "custom";
  });
  const key = field(
    "device-key",
    "Public key (optional)",
    h("input", { autocomplete: "off", spellcheck: "false", class: "secret" }),
    "An app that makes its own WireGuard key shows its public key: paste it here, and its private key never leaves the device. Leave it empty for Melete to make a whole config.",
  );
  const result = h("div");
  const made = form([name, kind, parts, key], "Add device", async () => {
    result.replaceChildren();
    const typed = name.input.value.trim();
    if (!typed) {
      name.fail("Give the device a name.");
      return null;
    }
    const choice = choices.find((c) => c.value === kind.input.value);
    const chosen = choice.parts ?? parts.picked();
    if (chosen.length === 0) {
      parts.fail("Choose at least one part.");
      return null;
    }
    const publicKey = key.input.value.trim() || null;
    let added;
    try {
      added = await api("/tunnel/devices", { method: "POST", body: { name: typed, parts: chosen, public_key: publicKey } });
    } catch (err) {
      if (err.status === 400 && publicKey) {
        key.fail("That isn't a WireGuard public key. Copy it again from the app: it's 44 characters ending in “=”.");
        return null;
      }
      if (err.status === 409 && publicKey && /public key/.test(err.message)) {
        key.fail("Another device already has this key. Make a new key pair in the app and paste its public key.");
        return null;
      }
      if (err.status === 429) throw new Error(`You have ${limit} devices, the most there can be. Remove one first.`);
      throw err;
    }
    name.input.value = key.input.value = "";
    result.replaceChildren(shownOnce(added));
    await refresh();
    return null;
  });
  return [made, result];
}

// What a device needs to connect, shown this once: its whole config, or for an app that made
// its own key, the preshared key and Melete's details.
function shownOnce(added) {
  const note = "Copy it now: it won't be shown again. If it's lost, remove the device and add it again.";
  if (added.wireguard_config) {
    return copyBox(`WireGuard config for “${added.name}”`, added.wireguard_config, `${note} Import it into a WireGuard app; it holds the device's private key, so keep it to that device.`);
  }
  const details = [
    `PresharedKey = ${added.preshared_key}`,
    `PublicKey = ${added.server_public_key}`,
    `Endpoint = ${added.endpoint}`,
    `AllowedIPs = ${added.server_address}/32`,
    `Address = ${added.address}/32`,
  ].join("\n");
  return copyBox(`Tunnel details for “${added.name}”`, details, `${note} Enter them in the app that gave the public key.`);
}

// Several lines to copy, with a Copy button.
function copyBox(label, value, note) {
  const id = `device-secret-${Math.random().toString(36).slice(2)}`;
  const text = h("textarea", { id, readonly: true, spellcheck: "false", class: "secret", rows: String(value.split("\n").length + 1) }, value);
  const copy = h("button", { type: "button", class: "quiet" }, "Copy");
  copy.addEventListener("click", async () => {
    text.select();
    try {
      await navigator.clipboard.writeText(value);
      copy.textContent = "Copied";
    } catch {
      // Clipboard access needs https; the text is selected for Ctrl+C instead.
      copy.textContent = "Selected; press Ctrl+C";
    }
  });
  requestAnimationFrame(() => {
    text.focus();
    text.select();
  });
  return h("div", { class: "secret-box", role: "status" }, h("label", { for: id }, label), h("div", { class: "secret-row" }, text, copy), h("p", { class: "note" }, note));
}
