// Outside services, for admins: TMDB, MusicBrainz, slskd, Deluge, the seedbox and the
// rest, each with its settings, a connection test that can try values before they're saved,
// and saving, which takes effect without a restart. A setting from .env wins and is shown, not
// changed; a secret is only ever said to be set.

import { field, form, group, secret } from "/web/forms.js";
import { SignedOut, announce, api, h } from "/web/lib.js";

const FROM = {
  environment: "Set in .env, which wins; change it there.",
  settings: null,
  default: null,
};

// One field's input, and how to read what's to be saved from it: a value, null to clear, or
// undefined to leave it as it is.
function input(service, f) {
  const id = `service-${f.name.toLowerCase()}`;
  const fixed = f.from === "environment";
  if (f.input.toggle) {
    const box = h("input", { type: "checkbox", id, checked: f.value === f.input.toggle.on, disabled: fixed });
    const el = h("div", { class: "check" }, box, h("label", { for: id }, f.label));
    return {
      box: h("div", { class: "field" }, el, fixed ? h("p", { class: "note" }, FROM.environment) : f.hint ? h("p", { class: "note" }, f.hint) : null),
      clear() {},
      fail() {},
      read: () => (fixed ? undefined : box.checked ? f.input.toggle.on : "off"),
    };
  }
  if (f.input.choice) {
    const { options } = f.input.choice;
    const current = f.value ?? options[0]?.value;
    const select = h("select", { disabled: fixed }, options.map((o) => h("option", { value: o.value, selected: o.value === current }, o.label)));
    const made = field(id, f.label, select, fixed ? FROM.environment : f.hint || null);
    // Only a change is sent, so saving the service's other settings doesn't judge every title again.
    return { ...made, read: () => (fixed || select.value === current ? undefined : select.value) };
  }
  if (f.input === "secret") {
    const hint = fixed ? FROM.environment : f.set ? `Set. Leave it empty to keep it.${f.hint ? ` ${f.hint}` : ""}` : f.hint || "Not set.";
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
    fixed ? FROM.environment : f.hint || null,
  );
  return { ...made, read: () => (fixed ? undefined : made.input.value.trim() || null) };
}

// The values to send: only what's changed or to be cleared.
function values(inputs) {
  const out = {};
  for (const [name, i] of inputs) {
    const value = i.read();
    if (value !== undefined) out[name] = value;
  }
  return out;
}

// One service's settings, test and save; Indexers → Soulseek shows slskd's.
export function serviceSection(ctx, s) {
  const inputs = s.fields.map((f) => [f.name, input(s, f)]);
  const tested = h("p", { class: "form-result", role: "status" });
  const test = h("button", { type: "button", class: "quiet" }, "Test connection");
  test.addEventListener("click", async () => {
    test.disabled = true;
    tested.classList.remove("error");
    tested.textContent = "Testing…";
    try {
      const said = await api(`/settings/services/${s.key}/test`, { method: "POST", body: { values: values(inputs) } });
      tested.textContent = said.message;
      tested.classList.toggle("error", !said.ok);
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      tested.textContent = err.message;
      tested.classList.add("error");
    } finally {
      test.disabled = false;
    }
  });
  const fields = inputs.map(([, i]) => i);
  const saving = form(fields, `Save ${s.name}`, async () => {
    const changed = values(inputs);
    if (!Object.keys(changed).length) return "Nothing to save.";
    await api(`/settings/services/${s.key}`, { method: "PUT", body: { values: changed } });
    announce(`${s.name} saved`);
    ctx.reload(`${s.name} saved, and in use now.`);
    return null;
  });
  saving.querySelector(".form-actions").append(test);
  // Everything set in .env: nothing here to save, only to test.
  if (s.fields.every((f) => f.from === "environment")) saving.querySelector("button[type=submit]").hidden = true;
  const extra = [];
  // slskd's key is one Melete and slskd share: it can be made here, then set in both.
  if (s.key === "slskd") {
    const key = inputs.find(([name]) => name === "MELETE_SLSKD_API_KEY")?.[1];
    if (key && !key.input.disabled) {
      const shown = h("div");
      const make = h("button", { type: "button", class: "quiet" }, "Make a key");
      make.addEventListener("click", async () => {
        make.disabled = true;
        try {
          const made = await api("/settings/keys", { method: "POST" });
          key.input.value = made.key;
          shown.replaceChildren(
            secret("New API key", made.key, "Put it in slskd's settings too, under web.authentication.api_keys, then save it here."),
          );
        } catch (err) {
          if (err instanceof SignedOut) return ctx.signedOut();
          shown.replaceChildren(h("p", { class: "error" }, err.message));
        } finally {
          make.disabled = false;
        }
      });
      extra.push(h("div", { class: "form-actions" }, make), shown);
    }
  }
  const id = `service-${s.key}`;
  return h(
    "section",
    { class: "subgroup", "aria-labelledby": id },
    h("h3", { id }, s.name, " ", h("span", { class: "badge" }, s.on ? "On" : "Off")),
    h("p", { class: "note" }, s.about),
    saving,
    tested,
    extra,
  );
}

// Where a service lives when it isn't here: slskd is Soulseek's, under Indexers; Deluge and
// the seedbox are System's Downloading.
const ELSEWHERE = {
  slskd: { href: "#/admin/indexers/soulseek", name: "Indexers, Soulseek" },
  downloads: { href: "#/admin/system/downloading", name: "System, Downloading" },
  seedbox: { href: "#/admin/system/downloading", name: "System, Downloading" },
};

export async function services(ctx) {
  const all = await api("/settings/services");
  if (!ctx.current()) return [];
  const moved = all.filter((s) => ELSEWHERE[s.key]);
  return [
    group(
      "Outside services",
      h(
        "p",
        { class: "note" },
        "Changes take effect when saved, without a restart: playback and scans carry on, and downloads and imports take up where they were. Anything set in .env wins over what's set here.",
      ),
      moved.map((s) => h("p", { class: "note" }, `${s.name} is set up under `, h("a", { href: ELSEWHERE[s.key].href }, ELSEWHERE[s.key].name), ".")),
      ...all.filter((s) => !ELSEWHERE[s.key]).map((s) => serviceSection(ctx, s)),
    ),
  ];
}
