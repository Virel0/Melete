// Inviting a guest through Melete's own WireGuard tunnel, from the Together panel: one guest
// outside the household gets a WireGuard config of their own and the link to open once it's on
// (`POST /watch-parties/{id}/tunnel-invites`). Offered only while the tunnel is on. The config
// holds the guest's private key: it's shown here while this page is open, until Done, and kept
// nowhere else.

import { SignedOut, api, h } from "/web/lib.js";

// Whether Melete's own tunnel is on, asked once a page load.
let on = null;
const tunnelOn = () => (on ??= api("/tunnel/devices").then((j) => Boolean(j.enabled)).catch(() => false));

// The tunnel invite made last, kept while the panel is drawn again for the party's changes.
let made = null;

// The button for the panel's actions, and where the invite shows. `said` takes errors.
export function tunnelInvite(party, ctx, said) {
  // The panel is drawn again at each change to the party: what had focus in the invite gets it
  // back once the new one is in place.
  const focused = document.activeElement?.closest?.(".tunnel-invite") ? document.activeElement : null;
  const refocus = focused && ((focused.id && `#${focused.id}`) || focused.textContent);
  const box = h("div", { class: "tunnel-invite" });
  const button = h("button", { type: "button", class: "quiet", hidden: true }, "Invite through the tunnel");
  tunnelOn().then((yes) => {
    button.hidden = !yes;
  });
  const draw = () => {
    if (made?.party !== party.id) return box.replaceChildren();
    box.replaceChildren(...shown(made, () => {
      made = null;
      draw();
      button.focus();
    }));
    if (made.fresh) {
      made.fresh = false;
      box.querySelector("strong")?.focus();
    }
  };
  button.addEventListener("click", async () => {
    button.disabled = true;
    try {
      const answer = await api(`/watch-parties/${party.id}/tunnel-invites`, { method: "POST" });
      made = { party: party.id, config: answer.wireguard_config, link: `${answer.server}${answer.path}`, fresh: true };
      draw();
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      said.textContent = err.message;
    } finally {
      button.disabled = false;
    }
  });
  draw();
  if (refocus) {
    requestAnimationFrame(() => {
      const again = refocus.startsWith("#") ? box.querySelector(refocus) : [...box.querySelectorAll("button, strong")].find((e) => e.textContent === refocus);
      again?.focus();
    });
  }
  return { button, box };
}

// The three steps for the guest, with the config and the link to send them, and Done.
function shown(m, done) {
  const note = h("p", { class: "note", role: "status" });
  const config = h("textarea", { id: "tunnel-invite-config", readonly: true, spellcheck: "false", class: "secret", rows: String(m.config.split("\n").length) }, m.config);
  const link = h("input", { id: "tunnel-invite-link", readonly: true, class: "secret", value: m.link });
  const download = h("button", { type: "button", class: "quiet" }, "Download config");
  download.addEventListener("click", () => {
    const url = URL.createObjectURL(new Blob([m.config], { type: "text/plain" }));
    const a = h("a", { href: url, download: "melete-party.conf" });
    document.body.append(a);
    a.click();
    a.remove();
    URL.revokeObjectURL(url);
    note.textContent = "Saved as melete-party.conf. Send it to the guest alone.";
  });
  const copyConfig = copyButton("Copy config", config, m.config, note);
  const copyLink = copyButton("Copy link", link, m.link, note);
  const finished = h("button", { type: "button", class: "quiet" }, "Done");
  finished.addEventListener("click", done);
  return [
    h("strong", { tabindex: "-1" }, "For one guest outside the household"),
    h(
      "ol",
      {},
      h("li", {}, "They install the WireGuard app on their phone or computer."),
      h("li", {}, "They import this config, from the file or pasted in.", h("div", { class: "secret-row" }, config), h("div", { class: "form-actions" }, download, copyConfig)),
      h("li", {}, "With the tunnel on, they open this link.", h("div", { class: "secret-row" }, link, copyLink)),
    ),
    h("p", { class: "note" }, "Send both to that one person: the config holds their key. It lets them in once, within a day, to this party alone, and ends when they leave or the party does. It isn't shown again after Done or once this page closes."),
    note,
    h("div", { class: "form-actions" }, finished),
  ];
}

function copyButton(label, field, text, note) {
  const button = h("button", { type: "button", class: "quiet" }, label);
  button.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(text);
      note.textContent = "Copied.";
    } catch {
      // Copying needs HTTPS; selected instead, for Ctrl+C.
      field.select();
      note.textContent = "Selected: press Ctrl+C to copy.";
    }
  });
  return button;
}
