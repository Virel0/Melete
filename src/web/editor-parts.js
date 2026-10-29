// What the quality profile and custom format editors share: ids unique on the page, a checkbox,
// the buttons at an editor's foot, and sending its form.

import { SignedOut, h } from "/web/lib.js";

let ids = 0;
export const uid = (base) => `${base}-${++ids}`;

export function check(id, label, checked) {
  const box = h("input", { type: "checkbox", id, checked });
  return { box, el: h("div", { class: "check" }, box, h("label", { for: id }, label)) };
}

// The buttons at the foot of an editor: save, and back without saving.
export function actions(saveLabel, onCancel) {
  const result = h("p", { class: "form-result", role: "status" });
  const save = h("button", { type: "submit" }, saveLabel);
  const cancel = h("button", { type: "button", class: "quiet", onclick: onCancel }, "Cancel");
  return { result, save, el: h("div", { class: "form-actions" }, save, cancel, result) };
}

// Sends an editor's form once at a time; `submit` returns the message for the list, or null
// when it marked a field wrong itself.
export function sending(formEl, foot, submit, done, ctx) {
  formEl.addEventListener("submit", async (event) => {
    event.preventDefault();
    foot.result.textContent = "";
    foot.result.classList.remove("error");
    for (const wrong of formEl.querySelectorAll("[aria-invalid=true]")) wrong.removeAttribute("aria-invalid");
    for (const error of formEl.querySelectorAll(".field-error")) error.hidden = true;
    foot.save.disabled = true;
    try {
      const message = await submit();
      if (message) return done(message);
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      foot.result.textContent = err.message;
      foot.result.classList.add("error");
    } finally {
      foot.save.disabled = false;
    }
    formEl.querySelector("[aria-invalid=true]")?.focus();
  });
}
