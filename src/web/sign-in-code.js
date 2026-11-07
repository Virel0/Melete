// Signing the web UI in with a code (Quick Connect), for a browser with no keyboard to speak
// of, such as a TV's: the page shows a code, someone signed in on another device enters it under
// Your account → Quick Connect and approves it, and this page is signed in as them, to the same
// cookie a password gives. The secret the page polls with stays in this page's memory; it goes
// to Melete in a request's body only, never in an address.

import { SignedOut, announce, api, h } from "/web/lib.js";

// How often the page asks whether the code was approved.
const POLL_MS = 3000;

// codeSignIn({ deviceName, signedIn, back }) → { el, focus(), stop() }: `signedIn` gets the
// user once the code is approved; `back` returns to the password form.
export function codeSignIn({ deviceName, signedIn, back }) {
  const s = { secret: null, timer: null, stopped: false };
  const heading = h("h2", { id: "code-sign-in-head", tabindex: "-1" }, "Sign in with a code");
  const body = h("div", { class: "code-sign-in-body" });
  const usePassword = h("button", { type: "button", class: "quiet" }, "Use a password instead");
  usePassword.addEventListener("click", () => {
    stop();
    back();
  });
  const el = h("section", { class: "code-sign-in", "aria-labelledby": "code-sign-in-head" }, heading, body, usePassword);

  function stop() {
    s.stopped = true;
    clearTimeout(s.timer);
    s.secret = null;
  }

  // Asks for a code and shows it; then waits for someone to approve it.
  async function start() {
    clearTimeout(s.timer);
    s.secret = null;
    body.replaceChildren(h("p", { class: "status", role: "status" }, "Getting a code…"));
    let started;
    try {
      started = await api("/auth/quick-connect", { method: "POST", body: { device_name: deviceName } });
    } catch (err) {
      if (s.stopped) return;
      return trouble(err.status === 429 ? "Too many codes are waiting right now. Try again in a few minutes." : err.message);
    }
    if (s.stopped) return;
    s.secret = started.secret;
    // Shown in two halves; read out a character at a time.
    const spelled = started.code.split("").join(" ");
    body.replaceChildren(
      h("p", {}, "On a phone or computer where you're signed in to Melete, open Your account, then Quick Connect, and enter this code:"),
      h(
        "p",
        { class: "quick-code" },
        h("span", { "aria-hidden": "true" }, `${started.code.slice(0, 3)} ${started.code.slice(3)}`),
        h("span", { class: "visually-hidden" }, spelled),
      ),
      h("p", { class: "note", role: "status" }, "Waiting for it to be approved. It lasts five minutes."),
    );
    announce(`Your code is ${spelled}`);
    s.timer = setTimeout(poll, POLL_MS);
  }

  // Whether the code was approved yet: signed in once it was, a new code offered once it ran out.
  async function poll() {
    if (s.stopped || !s.secret) return;
    let answer;
    try {
      answer = await api("/auth/quick-connect/web-poll", { method: "POST", body: { secret: s.secret } });
    } catch (err) {
      if (s.stopped) return;
      if (err instanceof SignedOut || err.status === 404) return trouble("That code ran out before anyone approved it.");
      // Melete didn't answer, say: keep asking, as the code may still be approved.
      s.timer = setTimeout(poll, POLL_MS * 2);
      return;
    }
    if (s.stopped) return;
    if (answer?.user) {
      stop();
      return signedIn(answer.user);
    }
    s.timer = setTimeout(poll, POLL_MS);
  }

  // Something went wrong: say what, and offer a new code.
  function trouble(message) {
    s.secret = null;
    const again = h("button", { type: "button" }, "Get a new code");
    again.addEventListener("click", start);
    body.replaceChildren(h("p", { class: "error", role: "alert" }, message), again);
    again.focus();
  }

  start();
  return { el, focus: () => heading.focus(), stop };
}
