// The VPN beside Melete (Gluetun), under Outside services: how it is (running or not, the
// address the outside world sees, the forwarded port) and starting, stopping or restarting it.
// Stopping and restarting ask first: what shares its network (Soulseek, the trailer downloader,
// services sent through its proxy) waits while it's down.

import { SignedOut, announce, api, h } from "/web/lib.js";
import { askFirst } from "/web/manage-parts.js";
import { stateLabel } from "/web/page-parts.js";

// What's known of it, as a short list.
function facts(view) {
  if (!view.set_up) return [h("p", { class: "note" }, "Set the control server's address and Melete's key above, and save, to see how it is.")];
  if (view.trouble) return [h("p", {}, stateLabel("failed", "Can't ask it")), h("p", { class: "error" }, view.trouble)];
  const s = view.state;
  const seen = s.public_ip ? `${s.public_ip}${s.country ? ` (${s.country})` : ""}` : "Not known yet";
  return [
    h("p", {}, s.running ? stateLabel("have", "Running") : stateLabel("off", "Stopped: what goes through it waits")),
    h(
      "dl",
      { class: "kv" },
      h("dt", {}, "The outside world sees"),
      h("dd", {}, s.running ? seen : "Nothing: the VPN is stopped"),
      h("dt", {}, "Forwarded port"),
      h("dd", {}, s.forwarded_port ? String(s.forwarded_port) : "None"),
    ),
  ];
}

export function vpnCard(ctx) {
  const shown = h("div", { role: "status" });
  const room = h("div");
  const buttons = h("div", { class: "form-actions" });
  const draw = (view) => {
    shown.replaceChildren(...facts(view));
    const ready = view.set_up && !view.trouble;
    const act = (label, action, ask) => {
      const b = h("button", { type: "button", class: "quiet", disabled: !ready }, label);
      const run = async () => {
        const after = await api(`/vpn/${action}`, { method: "POST" });
        room.replaceChildren();
        announce(`VPN: ${label.toLowerCase().replace("…", "")} done`);
        draw(after);
      };
      b.addEventListener("click", () => {
        if (!ask) {
          run().catch((err) => (err instanceof SignedOut ? ctx.signedOut() : room.replaceChildren(h("p", { class: "error", role: "alert" }, err.message))));
          return;
        }
        askFirst(room, { question: ask, confirm: label.replace("…", ""), keep: "Keep it running", back: b, signedOut: ctx.signedOut, run });
      });
      return b;
    };
    const running = view.state?.running;
    const startOrStop =
      running === false
        ? act("Start", "start")
        : act("Stop…", "stop", "Stop the VPN? Soulseek, the trailer downloader and the services sent through its proxy can't reach anything until it's started again; nothing goes direct meanwhile.");
    buttons.replaceChildren(
      startOrStop,
      act("Restart…", "restart", "Restart the VPN? What goes through it waits for a few seconds while it reconnects."),
      h("button", { type: "button", class: "quiet", onclick: () => load() }, "Ask again"),
    );
  };
  const load = async () => {
    shown.replaceChildren(h("p", { class: "note" }, "Asking Gluetun…"));
    try {
      draw(await api("/vpn"));
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      shown.replaceChildren(h("p", { class: "error" }, `Couldn't ask: ${err.message}`));
    }
  };
  load();
  return h("div", { class: "vpn-card" }, h("h4", {}, "How it is"), shown, buttons, room);
}
