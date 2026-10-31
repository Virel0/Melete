// Two-factor sign-in: the code step of signing in, and setting it up or turning it off on your
// account page. The codes come from an authenticator app (six digits, a new one every 30 s).

import { field, form, group, secret, when } from "/web/forms.js";
import { api, h } from "/web/lib.js";

// A six-digit code input that phones fill from an authenticator or offer a number pad for.
function codeInput() {
  return h("input", {
    inputmode: "numeric",
    autocomplete: "one-time-code",
    pattern: "[0-9]*",
    maxlength: "7",
    spellcheck: "false",
    class: "code",
  });
}

// What was typed, without spaces (some apps show "123 456"); null unless it's six digits.
const typedCode = (input) => {
  const code = input.value.replace(/\s/g, "");
  return /^[0-9]{6}$/.test(code) ? code : null;
};

// ---------- Signing in ----------

// The sign-in page's second step, after a right password: a code for `challenge`. `signedIn`
// gets the user; `restart` goes back to the password with a message.
export function codeStep({ challenge, signedIn, restart }) {
  const input = codeInput();
  input.id = "code-field";
  input.required = true;
  const error = h("p", { id: "code-error", class: "error", role: "alert" });
  const button = h("button", { type: "submit" }, "Sign in");
  const back = h("button", { type: "button", class: "quiet" }, "Use a different account");
  back.addEventListener("click", () => restart(null));
  const fail = (message) => {
    error.textContent = message;
    input.setAttribute("aria-invalid", "true");
    input.setAttribute("aria-describedby", "code-hint code-error");
    input.select();
    input.focus();
  };
  const el = h(
    "form",
    { novalidate: true },
    h(
      "div",
      {},
      h("label", { for: "code-field" }, "Code"),
      h("p", { class: "note", id: "code-hint" }, "Your account also asks for the six-digit code your authenticator app shows for Melete."),
      input,
    ),
    error,
    button,
    back,
    h("p", { class: "note" }, "Lost the device with the app? Whoever runs this server can remove the code step when they reset your password."),
  );
  input.setAttribute("aria-describedby", "code-hint");
  el.addEventListener("submit", async (event) => {
    event.preventDefault();
    error.textContent = "";
    const code = typedCode(input);
    if (!code) return fail("Enter the six digits your app shows.");
    button.disabled = true;
    button.textContent = "Signing in…";
    try {
      const answer = await api("/auth/web-login/second-factor", { method: "POST", body: { challenge, code } });
      signedIn(answer.user);
    } catch (err) {
      button.disabled = false;
      button.textContent = "Sign in";
      if (err.status === 410) return restart("That sign-in ran out. Enter your password again.");
      fail(err.data?.reason === "wrong_code" ? "That code didn't work. Enter the newest code your app shows for Melete." : err.message);
    }
  });
  return { form: el, focus: () => input.focus() };
}

// ---------- Your account ----------

// The account page's group: on since when and Turn off, or Set up when it's off.
export function twoFactorGroup(status, ctx) {
  const body = h("div");
  const say = (...nodes) => body.replaceChildren(...nodes);
  if (status.on) {
    const off = h("button", { type: "button", class: "quiet" }, "Turn off two-factor sign-in");
    off.addEventListener("click", () => {
      say(...turnOffForm(ctx));
      body.querySelector("input")?.focus();
    });
    say(
      h("p", {}, `On since ${when(status.on_since)}. Signing in with your password also asks for a code from your authenticator app.`),
      h("div", { class: "form-actions" }, off),
    );
  } else {
    say(
      h(
        "p",
        { class: "note" },
        "Off. Turn it on and signing in with your password also asks for a six-digit code from an authenticator app on your phone. Quick Connect, and devices already signed in, don't ask for one. Apps that can't ask for a code yet sign in with Quick Connect.",
      ),
      setUpForm(ctx, say),
    );
  }
  return group("Two-factor sign-in", body);
}

// Asking for the password, then showing the key and asking for a first code.
function setUpForm(ctx, say) {
  const password = field(
    "two-factor-password",
    "Your password",
    h("input", { type: "password", autocomplete: "current-password" }),
    "To set up two-factor sign-in, enter your password again.",
  );
  return form([password], "Set up two-factor sign-in", async () => {
    if (!password.input.value) {
      password.fail("Enter your password.");
      return null;
    }
    let setup;
    try {
      setup = await api("/me/two-factor/setup", { method: "POST", body: { password: password.input.value } });
    } catch (err) {
      if (err.data?.reason === "wrong_password") {
        password.fail("That isn't your password.");
        return null;
      }
      throw err;
    }
    say(...confirmStep(setup, ctx));
    return null;
  });
}

// The setup link as a QR code for the app to scan: the SVG the server drew, put in the page as
// it is (never loaded from a URL, which would put the secret in one). Null without one.
function qrCode(svg) {
  if (!svg) return null;
  const drawn = new DOMParser().parseFromString(svg, "image/svg+xml").documentElement;
  if (drawn.nodeName !== "svg") return null;
  const code = document.importNode(drawn, true);
  code.setAttribute("role", "img");
  code.setAttribute("aria-label", "QR code of the key, for an authenticator app to scan");
  return h("div", { class: "qr-code" }, code);
}

// The key, as a QR code, as text and as a link for the app, and the first code that turns it on.
function confirmStep(setup, ctx) {
  const code = field("two-factor-code", "Code from the app", codeInput(), "The six digits it shows for Melete now.");
  const confirm = form([code], "Turn on two-factor sign-in", async () => {
    const typed = typedCode(code.input);
    if (!typed) {
      code.fail("Enter the six digits your app shows.");
      return null;
    }
    try {
      await api("/me/two-factor/confirm", { method: "POST", body: { code: typed } });
    } catch (err) {
      if (err.data?.reason === "wrong_code") {
        code.fail("That code didn't work. Check you added the key above, then enter the newest code.");
        return null;
      }
      throw err;
    }
    ctx.reload("Two-factor sign-in is on. Your devices already signed in stay signed in: sign any out under Sessions and API keys.");
    return null;
  });
  const cancel = h("button", { type: "button", class: "quiet" }, "Cancel");
  cancel.addEventListener("click", () => ctx.reload("Two-factor sign-in is still off."));
  confirm.querySelector(".form-actions").append(cancel);
  const qr = qrCode(setup.qr);
  return [
    h(
      "p",
      {},
      qr
        ? "1. Add Melete to your authenticator app: scan this code with the app, type in the key, or open the link on the phone with the app."
        : "1. Add Melete to your authenticator app: type in this key, or open the link on the phone with the app.",
    ),
    h(
      "div",
      { class: "two-factor-key" },
      qr,
      h("div", {}, secret("Key", setup.secret, "Time-based, six digits, every 30 seconds. It's shown only until you turn two-factor sign-in on."), h("p", {}, h("a", { href: setup.uri }, "Open in an authenticator app"))),
    ),
    h("p", {}, "2. Enter the code the app shows, to check it works and turn it on."),
    confirm,
  ];
}

// Turning it off: the password and a current code, with Cancel.
function turnOffForm(ctx) {
  const password = field("two-factor-off-password", "Your password", h("input", { type: "password", autocomplete: "current-password" }));
  const code = field("two-factor-off-code", "Code from the app", codeInput());
  const turnOff = form([password, code], "Turn off two-factor sign-in", async () => {
    if (!password.input.value) {
      password.fail("Enter your password.");
      return null;
    }
    const typed = typedCode(code.input);
    if (!typed) {
      code.fail("Enter the six digits your app shows.");
      return null;
    }
    try {
      await api("/me/two-factor/off", { method: "POST", body: { password: password.input.value, code: typed } });
    } catch (err) {
      if (err.data?.reason === "wrong_code") {
        code.fail("That code didn't work. Enter the newest code your app shows for Melete.");
        return null;
      }
      if (err.data?.reason === "wrong_password") {
        password.fail("That isn't your password.");
        return null;
      }
      throw err;
    }
    ctx.reload("Two-factor sign-in is off: your password alone signs you in. Nothing was signed out.");
    return null;
  });
  const cancel = h("button", { type: "button", class: "quiet" }, "Cancel");
  cancel.addEventListener("click", () => ctx.reload());
  turnOff.querySelector(".form-actions").append(cancel);
  return [h("p", { class: "note" }, "Signing in will ask for your password alone. Your devices stay signed in."), turnOff];
}

// The user's changes to how they sign in, for the sessions group; nothing when there are none.
export function signInChanges(changes) {
  if (!changes.length) return null;
  return [
    h("h3", { class: "part-sub" }, "Changes to how you sign in"),
    h(
      "ul",
      { class: "rows" },
      changes.map((c) => h("li", {}, h("div", {}, ...describeChange(c)))),
    ),
  ];
}

// One entry of the record: what happened, and when and where from.
function describeChange(c) {
  if (c.change === "wrong_sign_in_code") {
    const what = c.count === 1 ? "A wrong code at sign-in" : `${c.count} wrong codes at sign-in`;
    return [
      h("strong", {}, what),
      h(
        "p",
        { class: "note" },
        `${c.count === 1 ? "" : "The last on "}${when(c.at)}. A code is asked for only after the right password: if that wasn't you, someone knows yours, so change it above.`,
      ),
    ];
  }
  const said = {
    two_factor_on: "Two-factor sign-in turned on",
    two_factor_off: "Two-factor sign-in turned off",
  };
  const where = { account: "from your account", server: "on the server" };
  return [h("strong", {}, said[c.change] ?? c.change), h("p", { class: "note" }, `${when(c.at)}, ${where[c.made_from] ?? c.made_from}.`)];
}
