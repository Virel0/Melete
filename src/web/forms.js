// The pieces settings forms are made of: labelled fields, forms that say what happened,
// questions asked in place before something is removed, secrets shown once; and the page
// frame they sit in, a side list of sections.

import { SignedOut, append, failure, h, loading, show } from "/web/lib.js";

// What to do when the session runs out mid-way; settingsPage sets it.
let signedOut = () => {};
export function onSignedOut(fn) {
  signedOut = fn;
}

const dateTime = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });
export const when = (unix) => (unix ? dateTime.format(new Date(unix * 1000)) : "never");

// A labelled input with room for a hint and an error, tied to it for screen readers.
export function field(id, label, input, hint) {
  input.id = id;
  const error = h("p", { class: "field-error", id: `${id}-error`, hidden: true });
  const hintEl = hint ? h("p", { class: "note", id: `${id}-hint` }, hint) : null;
  if (hint) input.setAttribute("aria-describedby", `${id}-hint`);
  const box = h("div", { class: "field" }, h("label", { for: id }, label), hintEl, input, error);
  return {
    box,
    input,
    fail(message) {
      error.textContent = message;
      error.hidden = false;
      input.setAttribute("aria-invalid", "true");
      input.setAttribute("aria-describedby", [hint ? `${id}-hint` : null, `${id}-error`].filter(Boolean).join(" "));
    },
    clear() {
      error.hidden = true;
      input.removeAttribute("aria-invalid");
      if (hint) input.setAttribute("aria-describedby", `${id}-hint`);
      else input.removeAttribute("aria-describedby");
    },
  };
}

// A form that sends once at a time and says what happened beside its button. `submit` returns
// the message to show, or throws; field errors it raises itself through `fail`.
export function form(fields, buttonLabel, submit) {
  const result = h("p", { class: "form-result", role: "status" });
  const button = h("button", { type: "submit" }, buttonLabel);
  const el = h("form", { novalidate: true }, fields.map((f) => f.box), h("div", { class: "form-actions" }, button, result));
  el.addEventListener("submit", async (event) => {
    event.preventDefault();
    for (const f of fields) f.clear();
    result.textContent = "";
    result.classList.remove("error");
    button.disabled = true;
    try {
      const message = await submit();
      if (message) result.textContent = message;
    } catch (err) {
      if (err instanceof SignedOut) return signedOut();
      result.textContent = err.message;
      result.classList.add("error");
    } finally {
      button.disabled = false;
      const invalid = el.querySelector("[aria-invalid=true]");
      invalid?.focus();
    }
  });
  return el;
}

// Swaps a row's actions for a question and two buttons; the safe one gets focus.
export function confirmInline(actions, { question, confirm, keep = "Cancel", run }) {
  const before = [...actions.childNodes];
  const restore = () => {
    actions.replaceChildren(...before);
    before.find((n) => n.tagName === "BUTTON")?.focus();
  };
  const go = h("button", { type: "button", class: "danger" }, confirm);
  const cancel = h("button", { type: "button", class: "quiet", onclick: restore }, keep);
  go.addEventListener("click", async () => {
    go.disabled = cancel.disabled = true;
    try {
      await run();
    } catch (err) {
      if (err instanceof SignedOut) return signedOut();
      actions.replaceChildren(h("span", { class: "error" }, err.message), " ", cancel);
      cancel.disabled = false;
      cancel.textContent = "Back";
      cancel.focus();
    }
  });
  actions.replaceChildren(h("span", { class: "question" }, question), go, cancel);
  cancel.focus();
}

// A secret shown once, selected so it can be copied straight away.
export function secret(label, value, note) {
  const id = `secret-${Math.random().toString(36).slice(2)}`;
  const input = h("input", { id, value, readonly: true, spellcheck: "false", class: "secret" });
  const copy = h("button", { type: "button", class: "quiet" }, "Copy");
  copy.addEventListener("click", async () => {
    input.select();
    try {
      await navigator.clipboard.writeText(value);
      copy.textContent = "Copied";
    } catch {
      // Clipboard access needs https; the text is selected for Ctrl+C instead.
      copy.textContent = "Selected; press Ctrl+C";
    }
  });
  const box = h("div", { class: "secret-box", role: "status" }, h("label", { for: id }, label), h("div", { class: "secret-row" }, input, copy), h("p", { class: "note" }, note));
  // Once it's on the page: focused, so it's read out, and selected, so it can be copied.
  requestAnimationFrame(() => {
    input.focus();
    input.select();
  });
  return box;
}

export function group(title, ...children) {
  const id = `group-${title.toLowerCase().replace(/[^a-z]+/g, "-")}`;
  return h("section", { class: "group", "aria-labelledby": id }, h("h2", { id }, title), children);
}

// A page of sections in a side list (Aoide's layout): Settings, and Wanted for admins. Each
// section's `page(ctx)` returns its groups; `ctx.reload(message)` draws it again with a message.
export async function sectioned({ title, base, sections }, section, ctx) {
  onSignedOut(ctx.signedOut);
  const allowed = sections.filter((s) => !s.admin || ctx.me.is_admin);
  const chosen = allowed.find((s) => s.key === section) ?? allowed[0];
  if (!chosen) {
    show(title, h("h1", {}, title), h("p", { class: "status" }, "This page is for admins."));
    return;
  }
  const nav = h(
    "nav",
    { class: "settings-nav", "aria-label": title },
    h(
      "ul",
      {},
      allowed.map((s) => h("li", {}, h("a", { href: `${base}/${s.key}`, "aria-current": s === chosen ? "page" : null }, s.name))),
    ),
  );
  const body = h("div", { class: "settings-body" }, loading(chosen.name.toLowerCase()));
  show(`${chosen.name} – ${title}`, h("h1", {}, title), h("div", { class: "settings" }, nav, body));
  if (section && section !== chosen.key) {
    body.replaceChildren(h("p", { class: "status" }, "That section is for admins."));
    return;
  }
  const fill = async (message) => {
    const parts = await chosen.page({ ...ctx, reload: (said) => fill(said).catch(failed) });
    if (!ctx.current()) return;
    body.replaceChildren();
    append(body, [message ? h("p", { class: "flash", role: "status" }, message) : null, ...parts]);
  };
  const failed = (err) => {
    if (err instanceof SignedOut) return ctx.signedOut();
    body.replaceChildren(failure(err, () => sectioned({ title, base, sections }, section, ctx)));
  };
  await fill().catch(failed);
}
