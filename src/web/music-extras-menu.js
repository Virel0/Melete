// The player's More menu, in the music bar and in the queue panel: Private listening, what
// you're doing, and the sleep timer (music-extras.js). A disclosure, as the video player's
// panels are: the button shows or hides the menu, Escape or a click elsewhere closes it, and
// Escape puts focus back on the button. Every copy follows every change, so the bar's and the
// panel's never disagree.
//
// extrasMenu({ below }) makes one, opening above its button, or below it with `below`.
// privateBadge() is the bar's sign that Private listening is on, which switches it off.

import { h } from "/web/lib.js";
import { icon } from "/web/music-controls.js";
import { ACTIVITIES, SLEEPS, activity, privately, setActivity, setPrivate, setSleep, sleeping } from "/web/music-extras.js";

let made = 0;

function sleepWords() {
  const sleep = sleeping();
  if (!sleep) return "";
  if (sleep.kind === "song") return "Pausing at the end of this song.";
  if (sleep.kind === "album") return "Pausing at the end of this album.";
  const minutes = Math.max(1, Math.ceil((sleep.until - Date.now()) / 60_000));
  return `Pausing in ${minutes} ${minutes === 1 ? "minute" : "minutes"}.`;
}

// A set of radio buttons, `name` unique to this copy.
function radios(legend, name, choices, chosen, pick) {
  const inputs = choices.map(([value, label]) => {
    const id = `${name}-${value}`;
    const input = h("input", { type: "radio", name, id, value, checked: value === chosen });
    input.addEventListener("change", () => input.checked && pick(value));
    return [input, h("div", { class: "check" }, input, h("label", { for: id }, label))];
  });
  const box = h("fieldset", { class: "extras-choices" }, h("legend", {}, legend), inputs.map(([, row]) => row));
  return { box, set: (value) => inputs.forEach(([input]) => (input.checked = input.value === value)) };
}

export function extrasMenu({ below = false } = {}) {
  const n = ++made;
  const id = `music-extras-${n}`;
  const button = h("button", { type: "button", class: "icon", "aria-label": "More", title: "More", "aria-expanded": "false", "aria-controls": id }, icon("more"));
  const secret = h("input", { type: "checkbox", id: `${id}-private`, "aria-describedby": `${id}-private-hint` });
  secret.addEventListener("change", () => setPrivate(secret.checked));
  const doing = radios("What you're doing", `${id}-activity`, [["none", "Nothing in particular"], ...ACTIVITIES], activity() ?? "none", (value) =>
    setActivity(value === "none" ? null : value),
  );
  const timer = radios("Sleep timer", `${id}-sleep`, SLEEPS, sleeping()?.kind ?? "off", setSleep);
  const left = h("p", { class: "note", role: "status" });
  const menu = h(
    "div",
    { class: `extras-menu${below ? " below" : ""}`, id, role: "group", "aria-label": "More", hidden: true },
    h("div", { class: "check" }, secret, h("label", { for: `${id}-private` }, "Private listening")),
    h("p", { class: "note check-hint", id: `${id}-private-hint` }, "While it's on, nothing you play is kept in your history or sent to Melete."),
    doing.box,
    timer.box,
    left,
  );
  const wrap = h("div", { class: "more-menu" }, button, menu);

  const show = (open) => {
    menu.hidden = !open;
    button.setAttribute("aria-expanded", String(open));
    if (open) (menu.querySelector("input:checked") ?? secret).focus();
  };
  button.addEventListener("click", () => show(menu.hidden));
  menu.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    event.preventDefault();
    event.stopPropagation();
    show(false);
    button.focus();
  });
  document.addEventListener("pointerdown", (event) => !menu.hidden && !wrap.contains(event.target) && show(false));
  const follow = () => {
    secret.checked = privately();
    doing.set(activity() ?? "none");
    timer.set(sleeping()?.kind ?? "off");
    left.textContent = sleepWords();
  };
  document.addEventListener("melete:extras", follow);
  // The minutes left, kept right while the menu is open.
  setInterval(() => !menu.hidden && sleeping()?.until && (left.textContent = sleepWords()), 15_000);
  follow();
  return wrap;
}

// Shown while Private listening is on; pressing it switches it off.
export function privateBadge() {
  const badge = h("button", { type: "button", class: "private-badge", title: "Private listening is on. Press to switch it off.", hidden: !privately() }, "Private");
  badge.setAttribute("aria-label", "Private listening is on. Switch it off");
  badge.addEventListener("click", () => setPrivate(false));
  document.addEventListener("melete:extras", () => (badge.hidden = !privately()));
  return badge;
}
