// HTTPS's certificate from Let's Encrypt, under Outside services, Certificates: the server used,
// the account's URI with the CAA records that let only it have certificates issued, the
// certificate in use, how renewing stands, and getting one now.

import { SignedOut, announce, api, h } from "/web/lib.js";
import { stateLabel } from "/web/page-parts.js";

const day = (unix) => new Date(unix * 1000).toLocaleDateString(undefined, { year: "numeric", month: "long", day: "numeric" });
const moment = (unix) => new Date(unix * 1000).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });

function facts(view) {
  if (!view.on) return [h("p", { class: "note" }, "Set the domain and the deSEC token above, and save, to have Melete get the certificate.")];
  const held = view.certificate;
  const shown = [];
  if (view.working) shown.push(h("p", {}, stateLabel("waiting", "Getting a certificate…")));
  else if (!held) shown.push(h("p", {}, stateLabel("waiting", "No certificate yet")));
  else if (!held.from_this_ca) shown.push(h("p", {}, stateLabel("waiting", "The one held is from the other server; a new one is on its way")));
  else shown.push(h("p", {}, stateLabel("have", view.staging ? "Got one from the staging server (no browser trusts it)" : "Got one")));
  if (!view.https) shown.push(h("p", { class: "note" }, "HTTPS isn't listening: set MELETE_TLS_BIND in .env and restart Melete to serve it."));
  const rows = [h("dt", {}, "For"), h("dd", {}, view.names.join(", ")), h("dt", {}, "Server"), h("dd", {}, view.staging ? "Let's Encrypt's staging server" : "Let's Encrypt")];
  if (held) rows.push(h("dt", {}, "Runs out"), h("dd", {}, day(held.not_after)));
  if (view.renew_at && !view.failures) rows.push(h("dt", {}, "Renewed"), h("dd", {}, `Around ${moment(view.renew_at)}`));
  if (view.failures) rows.push(h("dt", {}, "Tried again"), h("dd", {}, view.retry_at ? moment(view.retry_at) : "Soon"));
  shown.push(h("dl", { class: "kv" }, rows));
  if (view.failures) shown.push(h("p", { class: "error" }, `The last ${view.failures === 1 ? "attempt" : `${view.failures} attempts`} failed: ${view.last_error}`));
  if (view.account_uri) {
    shown.push(
      h("h4", {}, "Only this account"),
      h("p", { class: "note" }, "A CAA record lets only Melete's account have certificates issued for the domain. Add these to the zone at deSEC (the account on the real server is another one than staging's):"),
      h("pre", { class: "report-details", tabindex: "0" }, view.caa.join("\n")),
    );
  }
  return shown;
}

export function certificateCard(ctx) {
  const shown = h("div", { role: "status" });
  const room = h("div");
  const now = h("button", { type: "button", class: "quiet" }, "Get one now");
  now.addEventListener("click", async () => {
    now.disabled = true;
    try {
      draw(await api("/certificates/renew", { method: "POST" }));
      announce("Getting a certificate");
      setTimeout(load, 15000);
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      room.replaceChildren(h("p", { class: "error", role: "alert" }, err.message));
    }
  });
  const again = h("button", { type: "button", class: "quiet", onclick: () => load() }, "Look again");
  const draw = (view) => {
    room.replaceChildren();
    shown.replaceChildren(...facts(view));
    now.disabled = !view.on || view.working;
  };
  const load = async () => {
    try {
      draw(await api("/certificates"));
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      shown.replaceChildren(h("p", { class: "error" }, `Couldn't ask: ${err.message}`));
    }
  };
  load();
  return h("div", { class: "vpn-card" }, h("h4", {}, "How it stands"), shown, h("div", { class: "form-actions" }, now, again), room);
}
