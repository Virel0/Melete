// Outside services and Melete's own features, for admins. Outside services (TMDB, MusicBrainz,
// the proxy, alerts and the rest) and Features (scans, seek-bar previews, Auto DJ…) each list
// theirs group by group, one line each with its state, and each opens its own page: its
// settings, a connection test that can try values before they're saved, and saving, which takes
// effect without a restart. The server says which group and page each is in. A setting from .env
// wins and is shown, not changed, naming its variable and the way to unlock it; a secret is only
// ever said to be set.

import { form, group, secret } from "/web/forms.js";
import { SignedOut, announce, api, h } from "/web/lib.js";
import { emptyState, stateLabel } from "/web/page-parts.js";
import { input, values } from "/web/service-fields.js";
import { certificateCard } from "/web/certificate-card.js";
import { vpnCard } from "/web/vpn-card.js";

// The pages, by the name the server gives them.
const PAGES = { services: "Outside services", features: "Features" };

// "an address", "a host": what a setting is called, after "Needs".
function aOrAn(label) {
  const words = /^[A-Z][a-z]/.test(label) ? label[0].toLowerCase() + label.slice(1) : label;
  return /address/i.test(words) ? "an address" : `${/^[aeiouAEIOU]/.test(words) ? "an" : "a"} ${words}`;
}

// What a service is waiting for when it's off though something of it is set: the first of its
// settings to type in (its address, key or host), while that one isn't set.
function missing(s) {
  const started = s.fields.some((f) => (f.input.toggle ? f.value === f.input.toggle.on : f.set));
  const first = s.fields.find((f) => !f.input.toggle && !f.input.choice);
  return started && first && !first.set ? aOrAn(first.label) : null;
}

// A service's state, as [the state's mark, its words]: what it needs first, then a lock by
// .env, then on or off. `all` holds the service it needs, when there's one.
export function stateOf(s, all = []) {
  const needed = s.needs ? all.find((n) => n.key === s.needs) : null;
  if (needed && !needed.on) return ["waiting", `Needs ${needed.name}`];
  const turnedOn = s.fields.some((f) => f.input.toggle && f.value === f.input.toggle.on);
  if (s.component && !s.component.set_up && turnedOn) return ["waiting", `Needs the ${s.component.name}`];
  if (s.fields.length && s.fields.every((f) => f.from === "environment")) return ["notyet", "Locked by .env"];
  if (s.on) return ["have", "On"];
  const wants = missing(s);
  return wants ? ["waiting", `Needs ${wants}`] : ["off", "Off"];
}

const chip = (s, all) => stateLabel(...stateOf(s, all));

// Why a feature can't be turned on while the service it needs is off.
const NEEDS_WHY = {
  tmdb: "Needs TMDB's API Read Access Token, which isn't set. Set it above and save TMDB, then this can be turned on.",
};

// A feature whose service is off: its settings shown as they are, but nothing to change, save
// or test until that one is set up; `why` is the paragraph saying so.
function blocked(inputs, saving, test, why) {
  for (const [, i] of inputs) {
    i.input.disabled = true;
    i.input.setAttribute("aria-describedby", [why.id, i.input.getAttribute("aria-describedby")].filter(Boolean).join(" "));
    i.read = () => undefined;
  }
  saving.querySelector("button[type=submit]").hidden = true;
  test.hidden = true;
}

// One service's settings, test and save, under its name, state and purpose, with what leaves
// the server folded under them; its own page draws it, and Indexers → Soulseek shows slskd's. A
// feature that only works with another service (`needed`) is drawn under that one's, a heading
// lower, and is blocked while that one is off; `features` are the ones drawn under this one.
// `heading` is the heading's level, and `note` a paragraph to put above the settings.
export function serviceSection(ctx, s, { needed = null, features = [], heading = needed ? "h4" : "h3", note = null } = {}) {
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
  // A setting of Melete's own has nothing to connect to, so no test.
  if (s.testable) saving.querySelector(".form-actions").append(test);
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
  // The VPN's state and its start, stop and restart, under its settings.
  if (s.key === "vpn") extra.push(vpnCard(ctx));
  // The certificate's state, the account's CAA records, and getting one now.
  if (s.key === "certificates") extra.push(certificateCard(ctx));
  const id = `service-${s.key}`;
  const why = needed && !needed.on ? h("p", { class: "note", id: `${id}-needs` }, NEEDS_WHY[needed.key] ?? `Needs ${needed.name}, which is off. Set it up above, then this can be turned on.`) : null;
  if (why) blocked(inputs, saving, test, why);
  return h(
    "section",
    { class: heading === "h2" ? "group" : needed ? "subgroup feature" : "subgroup", "aria-labelledby": id },
    h(heading, { id, class: "service-heading" }, s.name, " ", chip(s, needed ? [needed] : [])),
    h("p", {}, s.purpose),
    h("details", { class: "service-about" }, h("summary", {}, "What leaves the server"), h("p", { class: "note" }, s.about)),
    note,
    why,
    s.component && h("p", { class: "note" }, s.component.set_up ? `The ${s.component.name} is set up.` : `The ${s.component.name} isn't set up, so this can't run yet.`),
    saving,
    s.testable && tested,
    extra,
    features.length
      ? [
          h("p", { class: "note" }, `Features that use ${s.name}, each with its own switch:`),
          features.map((f) => serviceSection(ctx, f, { needed: s })),
        ]
      : null,
  );
}

// Where a service lives when it isn't here: slskd is Soulseek's, under Indexers; Deluge and
// the seedbox are System's Downloading.
const ELSEWHERE = {
  slskd: { href: "#/admin/indexers/soulseek", name: "Indexers, Soulseek" },
  downloads: { href: "#/admin/system/downloading", name: "System, Downloading" },
  seedbox: { href: "#/admin/system/downloading", name: "System, Downloading" },
};

// Where a service is set up: its own page, the page of the one it needs (a feature of TMDB's
// is on TMDB's), or where it lives when it isn't here.
export function serviceHref(s) {
  return ELSEWHERE[s.key]?.href ?? `#/admin/system/${s.page}/${s.needs ?? s.key}`;
}

// One service's line in the list: its name, what it's for and its state, the whole line a
// link to its page, with a chevron saying so.
function serviceRow(s) {
  const away = ELSEWHERE[s.key];
  return h(
    "li",
    {},
    h(
      "a",
      { class: "service-row", href: serviceHref(s) },
      h("span", { class: "service-name" }, s.name),
      h("span", { class: "service-purpose" }, away ? `${s.purpose}. Set up under ${away.name}.` : s.purpose),
      chip(s),
      h("span", { class: "service-go", "aria-hidden": "true" }, "›"),
    ),
  );
}

// Why a setting is locked, on a page that has one.
const LOCKED = [
  "A setting here that is also given in .env is locked, since .env wins, and names its variable. To set it here, remove that line from .env and restart Melete with ",
  h("code", {}, "docker compose up -d"),
  " (",
  h("code", {}, "docker compose restart"),
  " keeps the old values).",
];

// One service's own page: back to the list, then its settings and the features that use it.
function servicePage(ctx, all, page) {
  const asked = all.find((s) => s.key === ctx.id);
  // A feature that needs another service is set up on that one's page.
  const s = asked?.needs ? all.find((n) => n.key === asked.needs) : asked;
  const home = s?.page ?? page;
  const back = h("p", { class: "back" }, h("a", { href: `#/admin/system/${home}` }, `Back to ${PAGES[home]}`));
  if (!s) return [back, emptyState(`There's no service called “${ctx.id}”.`, { href: `#/admin/system/${page}`, label: `See ${PAGES[page]}` })];
  const away = ELSEWHERE[s.key];
  if (away) return [back, h("p", {}, `${s.name} is set up under `, h("a", { href: away.href }, away.name), ".")];
  const locked = s.fields.some((f) => f.from === "environment") ? h("p", { class: "note" }, ...LOCKED.map((part) => (typeof part === "string" ? part : part.cloneNode(true)))) : null;
  return [back, serviceSection(ctx, s, { heading: "h2", note: locked, features: all.filter((f) => f.needs === s.key) })];
}

// What each page says above its list.
const INTRO = {
  services:
    ".env holds what Melete needs before it starts, and is the only place to set it: its folders (libraries, data, transcoding), the address it listens on, the ways in, and the helpers running beside it. Each service below is set up on its own page instead, and takes effect when saved, without a restart: playback and scans carry on, and downloads and imports take up where they were.",
  features: "Melete's own features, worked out on the server: none of them sends anything outside it. Each is set up on its own page, and takes effect when saved, without a restart.",
};

// A page's list, group by group in the server's order, or with `ctx.id` one service's page.
async function listing(ctx, page) {
  const all = await api("/settings/services");
  if (!ctx.current()) return [];
  if (ctx.id) return servicePage(ctx, all, page);
  // A feature that needs another service is listed with that one, on its page.
  const listed = all.filter((s) => s.page === page && !s.needs);
  const groups = [];
  for (const s of listed) {
    if (groups.at(-1)?.name !== s.group) groups.push({ name: s.group, items: [] });
    groups.at(-1).items.push(s);
  }
  const on = listed.filter((s) => s.on).length;
  return [
    h("p", { class: "service-summary" }, `${on} of ${listed.length} on.`),
    h("p", { class: "note" }, INTRO[page]),
    // In each group, the services set up elsewhere come last.
    groups.map((g) =>
      group(g.name, h("ul", { class: "service-rows" }, [...g.items.filter((s) => !ELSEWHERE[s.key]), ...g.items.filter((s) => ELSEWHERE[s.key])].map(serviceRow))),
    ),
  ];
}

export const services = (ctx) => listing(ctx, "services");
export const features = (ctx) => listing(ctx, "features");
