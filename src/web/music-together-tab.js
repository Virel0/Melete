// The Together tab of the Now playing panel: Listen Together (music-together.js) from the web.
// Out of a session: start one with what's playing, or join one with its code. In one: its code
// to share, who started it, who's in it (each person and device, this one marked), Leave, and,
// for whoever started it or an admin (as the server allows), End for everyone, asked twice.
// Joining needs every music library; anyone limited to some is told so, not refused.
//
// togetherTab() → { body }: it draws itself on each `melete:together` event.

import { SignedOut, h } from "/web/lib.js";
import { avatar } from "/web/profile-picture.js";
import { seesAllMusic } from "/web/music-access.js";
import { endSession, joinSession, leaveSession, mayEnd, startSession, together } from "/web/music-together.js";

const CLOSED = "Listening together isn't open to you, as you can see only some music libraries. An admin can change what you see.";

// Runs a button's action: the button waits, and a refusal is said under it.
async function act(button, said, action) {
  button.disabled = true;
  said.textContent = "";
  try {
    await action();
  } catch (err) {
    if (err instanceof SignedOut) return;
    said.textContent = err.message;
    button.disabled = false;
  }
}

function outside(note) {
  const said = h("p", { class: "error", role: "alert" });
  const start = h("button", { type: "button" }, "Start a session");
  start.addEventListener("click", () => act(start, said, startSession));
  const code = h("input", {
    id: "together-code",
    name: "code",
    autocomplete: "off",
    autocapitalize: "characters",
    spellcheck: "false",
    maxlength: "9",
    required: true,
    "aria-describedby": "together-code-note",
  });
  const join = h("button", { type: "submit", class: "quiet" }, "Join");
  const form = h(
    "form",
    { class: "together-join" },
    h("label", { for: "together-code" }, "Join with a code"),
    h("p", { class: "note", id: "together-code-note" }, "Six letters and digits, from someone in the session."),
    h("div", { class: "together-code-row" }, code, join),
  );
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    if (!code.value.trim()) {
      said.textContent = "Type the session's code.";
      return code.focus();
    }
    act(join, said, () => joinSession(code.value));
  });
  return [
    note ? h("p", { class: "note", role: "status" }, note) : null,
    h("p", {}, "Play music in step with others in the household, each on their own device. Start a session with what's playing here, or join one."),
    start,
    form,
    said,
  ];
}

function member(m) {
  return h(
    "li",
    {},
    avatar(m, "small"),
    h("span", { class: "who" }, h("span", {}, h("strong", {}, m.username), m.you ? " (this device)" : null), h("small", {}, m.device)),
  );
}

function inside(session, note) {
  const said = h("p", { class: "error", role: "alert" });
  const leave = h("button", { type: "button", class: "quiet" }, "Leave");
  leave.addEventListener("click", () => act(leave, said, leaveSession));
  const asking = h("div", { class: "together-ask", hidden: true });
  const end = h("button", { type: "button", class: "quiet" }, "End for everyone");
  end.addEventListener("click", () => {
    const yes = h("button", { type: "button", class: "danger" }, "End it");
    const no = h("button", { type: "button", class: "quiet" }, "Keep it");
    yes.addEventListener("click", () => act(yes, said, endSession));
    no.addEventListener("click", () => {
      asking.hidden = true;
      end.hidden = false;
      end.focus();
    });
    asking.replaceChildren(h("p", { class: "question" }, "End the session for everyone in it? Their music stops following this one."), h("div", { class: "actions" }, yes, no));
    asking.hidden = false;
    end.hidden = true;
    no.focus();
  });
  const spelt = session.code.split("").join(" ");
  return [
    h("p", { class: "kind" }, "Session code"),
    h("p", { class: "together-code" }, h("span", { "aria-hidden": "true" }, session.code), h("span", { class: "visually-hidden" }, spelt)),
    h("p", { class: "note" }, `Started by ${session.host}. Anyone in the household who can see every music library can join with this code. Anyone in the session can change what plays.`),
    note ? h("p", { class: "note", role: "status" }, note) : null,
    h("h3", { id: "together-members" }, "In this session"),
    h("ul", { class: "together-members", "aria-labelledby": "together-members" }, session.members.map(member)),
    h("div", { class: "actions" }, leave, mayEnd() ? end : null),
    asking,
    said,
  ];
}

export function togetherTab() {
  const body = h("div", { class: "together" });
  let shownSession = null;
  const draw = () => {
    const { session, note } = together();
    // Keep the keyboard's place: a redraw for the same session in the same state changes nothing.
    const key = JSON.stringify(session ? [session.id, session.code, session.host, session.members, note] : ["out", note]);
    if (key === shownSession) return;
    const hadFocus = body.contains(document.activeElement);
    shownSession = key;
    if (!seesAllMusic()) return body.replaceChildren(h("p", { class: "note" }, CLOSED));
    body.replaceChildren(...(session ? inside(session, note) : outside(note)).filter(Boolean));
    if (hadFocus) (body.querySelector("button:not([hidden]), input") ?? body).focus();
  };
  document.addEventListener("melete:together", draw);
  draw();
  return { body };
}
