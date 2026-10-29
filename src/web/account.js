// Your account: password, languages, signing in through an identity provider, sessions and API
// keys, and Quick Connect for signing in another device. Every user has this page.

import { confirmInline, field, form, group, secret, when } from "/web/forms.js";
import { languagesGroup } from "/web/languages.js";
import { SignedOut, announce, api, h, oidcOutcome } from "/web/lib.js";

export async function account(ctx) {
  const [tokens, provider, link, languages] = await Promise.all([
    api("/auth/tokens"),
    api("/auth/oidc"),
    api("/auth/oidc/link"),
    languagesGroup(),
  ]);
  if (!ctx.current()) return [];

  const sessions = tokenList();
  const password = passwordForm(sessions.refresh);
  sessions.draw(tokens);
  const key = keyForm(sessions.refresh);
  const quick = quickConnect(ctx, sessions.refresh);
  const linking = provider.enabled ? providerGroup(provider, link, ctx) : null;

  return [
    group("Password", password),
    languages,
    linking,
    group("Sessions and API keys", h("p", { class: "note" }, "Each device you sign in on, and each key, until it's signed out or goes unused for 90 days."), sessions.list),
    group("Quick Connect", h("p", { class: "note" }, "Sign in a TV or a new device without typing your password on it: choose Quick Connect there and enter the code it shows."), ...quick),
    group("New API key", ...key),
  ];
}

// Changing your password; `refreshTokens` redraws the sessions, which it signs out.
function passwordForm(refreshTokens) {
  const current = field("current-password", "Current password", h("input", { type: "password", autocomplete: "current-password" }));
  const next = field(
    "new-password",
    "New password",
    h("input", { type: "password", autocomplete: "new-password" }),
    "At least 8 characters. Your other sessions and API keys are signed out when it changes.",
  );
  return form([current, next], "Change password", async () => {
    if (!current.input.value) {
      current.fail("Enter your current password.");
      return null;
    }
    if (next.input.value.length < 8) {
      next.fail("Use at least 8 characters.");
      return null;
    }
    try {
      await api("/me/password", {
        method: "PUT",
        body: { current_password: current.input.value, new_password: next.input.value },
      });
    } catch (err) {
      if (err.status === 403) {
      current.fail("That isn't your current password.");
      return null;
    }
      throw err;
    }
    current.input.value = next.input.value = "";
    refreshTokens();
    return "Password changed. Your other sessions and API keys were signed out.";
  });
}

// Your sessions and API keys: the list, `draw` for given ones, and `refresh` to ask again.
function tokenList() {
  const list = h("ul", { class: "rows" });
  const refreshTokens = async () => {
    const fresh = await api("/auth/tokens");
    drawTokens(fresh);
  };
  const drawTokens = (items) => {
    list.replaceChildren(...items.map((t) => tokenRow(t, list, refreshTokens)));
  };
  return { list, draw: drawTokens, refresh: refreshTokens };
}

// A session or API key, with Sign out or Revoke unless it's this browser's.
function tokenRow(t, list, refreshTokens) {
  const actions = h("div", { class: "row-actions" });
  if (!t.current) {
    const revoke = h("button", { type: "button", class: "quiet" }, t.kind === "session" ? "Sign out" : "Revoke");
    revoke.addEventListener("click", () =>
      confirmInline(actions, {
        question:
          t.kind === "session" ? `Sign out “${t.name}”?` : `Revoke “${t.name}”? Anything using this key stops working at once.`,
        confirm: t.kind === "session" ? "Sign it out" : "Revoke key",
        run: async () => {
          await api(`/auth/tokens/${t.id}`, { method: "DELETE" });
          announce(t.kind === "session" ? `Signed out ${t.name}` : `Revoked ${t.name}`);
          await refreshTokens();
          list.querySelector("button")?.focus();
        },
      }),
    );
    actions.append(revoke);
  }
  return h(
    "li",
    {},
    h(
      "div",
      {},
      h("strong", {}, t.name),
      " ",
      t.current ? h("span", { class: "badge" }, "This browser") : null,
      h(
        "p",
        { class: "note" },
        `${t.kind === "session" ? "Session" : `API key: ${(SCOPES[t.scope] ?? t.scope).toLowerCase()}`}. Created ${when(t.created_at)}; last used ${when(t.last_used_at)}.`,
      ),
    ),
    actions,
  );
}

// What a key may do, by its scope, in the words the form and the list use (and Users').
export const SCOPES = {
  read: "Read only",
  playback: "Read and play",
  "music-sync": "Music sync",
  full: "Everything you can do",
};

// Creating an API key, shown once; returns the form and where the key appears.
function keyForm(refreshTokens) {
  const keyName = field("key-name", "Name", h("input", { autocomplete: "off" }), "What it's for, e.g. “Home Assistant”.");
  const keyScope = field(
    "key-scope",
    "What it may do",
    h("select", {}, Object.entries(SCOPES).map(([value, label]) => h("option", { value }, label))),
    "Read only: browse and read your own data, change nothing. Read and play: that, plus playing and marking progress and what's watched. Music sync: Aoide's sync, nothing else. Everything you can do: as you, admin pages included if you're an admin. Choose the least it needs.",
  );
  const keyResult = h("div");
  const made = form([keyName, keyScope], "Create API key", async () => {
    const name = keyName.input.value.trim();
    if (!name) {
      keyName.fail("Give the key a name.");
      return null;
    }
    const scope = keyScope.input.value;
    const key = await api("/auth/keys", { method: "POST", body: { name, scope } });
    keyName.input.value = "";
    keyResult.replaceChildren(
      secret(
        `API key “${key.name}” (${SCOPES[key.scope].toLowerCase()})`,
        key.token,
        "Copy it now: it won't be shown again. Send it as Authorization: Bearer <key>.",
      ),
    );
    refreshTokens();
    return null;
  });
  return [made, keyResult];
}

// Quick Connect: the code form, and where the device it names is shown to sign in or not.
function quickConnect(ctx, refreshTokens) {
  const code = field(
    "quick-code",
    "Code",
    h("input", { autocomplete: "off", autocapitalize: "characters", spellcheck: "false", maxlength: "8" }),
    "The six letters and digits the device shows. Only enter a code from a device you're signing in yourself: it's signed in as you.",
  );
  // Two steps: look the code up and see which device it is, then sign it in or not.
  const quickCheck = h("div", { class: "quick-check" });
  const quickForm = form([code], "Look it up", async () => {
    quickCheck.replaceChildren();
    const typed = code.input.value.replace(/[\s-]/g, "");
    if (typed.length !== 6) {
      code.fail("Enter all six letters and digits.");
      return null;
    }
    let seen;
    try {
      seen = await api("/auth/quick-connect/lookup", { method: "POST", body: { code: typed } });
    } catch (err) {
      if (err.status === 404) {
        code.fail("No device is showing that code. Check it, or start again on the device: codes last five minutes.");
        return null;
      }
      throw err;
    }
    askToSignIn(ctx, { seen, typed, code, quickCheck, refreshTokens });
    return null;
  });
  return [quickForm, quickCheck];
}

// The device a code belongs to, with Sign in and Cancel.
function askToSignIn(ctx, { seen, typed, code, quickCheck, refreshTokens }) {
  const ago = (seconds) => (seconds < 60 ? "just now" : `${Math.round(seconds / 60)} min ago`);
  const result = h("p", { class: "form-result", role: "status" });
  const approve = h("button", { type: "button" }, `Sign in “${seen.device_name}”`);
  const cancel = h("button", { type: "button", class: "quiet" }, "Cancel");
  cancel.addEventListener("click", () => {
    quickCheck.replaceChildren();
    code.input.focus();
  });
  approve.addEventListener("click", async () => {
    approve.disabled = cancel.disabled = true;
    try {
      const approved = await api("/auth/quick-connect/approve", { method: "POST", body: { code: typed } });
      code.input.value = "";
      quickCheck.replaceChildren(h("p", { class: "flash", role: "status" }, `Signed in “${approved.device_name}” as you. It's in your sessions above.`));
      refreshTokens();
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      result.textContent = err.status === 404 ? "That code has run out. Start again on the device." : err.message;
      result.classList.add("error");
      cancel.disabled = false;
    }
  });
  quickCheck.replaceChildren(
    h("p", {}, h("strong", {}, seen.device_name), ` asked from ${seen.address}, ${ago(seen.age_seconds)}.`),
    h("p", { class: "note" }, "Sign it in only if that's the device in front of you: it will be signed in as you."),
    h("div", { class: "form-actions" }, approve, cancel, result),
  );
  approve.focus();
}

// What linking came back saying, shown once on the account page it returns to.
const LINK_OUTCOMES = {
  linked: "Your account is linked: you can sign in through the provider now.",
  taken: "That account at the provider is linked to someone else here already, so it wasn't linked to you.",
  denied: "You didn't sign in at the provider, so nothing was linked.",
  expired: "That ran out before you came back. Try again.",
  browser: "That was started in another browser. Try again here.",
  failed: "Linking didn't work. Try again; if it keeps failing, an admin can test the provider under Outside services.",
};

// Linking your account to your identity at the identity provider, or unlinking it.
function providerGroup(provider, link, ctx) {
  const outcome = oidcOutcome();
  const said = LINK_OUTCOMES[outcome];
  const flash = said ? h("p", { class: outcome === "linked" ? "flash" : "flash error", role: "status" }, said) : null;
  const actions = h("div", { class: "row-actions" });
  // Linking asks for the password again: a session alone can't add a way in to the account.
  const password = field(
    "link-password",
    "Your password",
    h("input", { type: "password", autocomplete: "current-password" }),
    `To link your ${provider.name} account, enter your Melete password again.`,
  );
  const linkForm = form([password], link.linked ? "Link a different account" : `Link your ${provider.name} account`, async () => {
    if (!password.input.value) {
      password.fail("Enter your password.");
      return null;
    }
    let started;
    try {
      started = await api("/auth/oidc/link", { method: "POST", body: { password: password.input.value } });
    } catch (err) {
      if (err.status === 403) {
        password.fail("That isn't your password.");
        return null;
      }
      throw err;
    }
    location.assign(started.authorize_url);
    return "Taking you to the provider…";
  });
  if (link.linked) {
    const unlink = h("button", { type: "button", class: "quiet" }, "Unlink");
    unlink.addEventListener("click", () =>
      confirmInline(actions, {
        question: `Unlink your ${provider.name} account? You'll sign in with your password until you link it again.`,
        confirm: "Unlink",
        run: async () => {
          await api("/auth/oidc/link", { method: "DELETE" });
          ctx.reload("Unlinked. Sign in with your password from now on.");
        },
      }),
    );
    actions.append(unlink);
  }
  const state = link.linked
    ? `Linked on ${when(link.linked_at)}: signing in through ${provider.name} signs in as you.`
    : `Not linked. Link it once, here, and you can sign in through ${provider.name} instead of typing your password.`;
  return group(`Sign in with ${provider.name}`, flash, h("p", { class: "note" }, state), linkForm, actions);
}
