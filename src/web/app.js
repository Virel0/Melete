// Melete's web UI: one page, routed by the URL's #fragment, talking to /api/v1 signed in by
// cookie. Everything shown is built with DOM nodes and textContent, never parsed markup, so a
// file or title name can't become HTML.

import { SignedOut, announce, api, failure, h, oidcOutcome, show } from "/web/lib.js";
import { adminPage } from "/web/admin.js";
import { kindPage, oldSeriesList } from "/web/watch-kind.js";
import { home } from "/web/home.js";
import { startBar } from "/web/music-bar.js";
import { pause as pauseMusic, startPlayer, stopPlayer } from "/web/music-player.js";
import { musicPage } from "/web/music.js";
import { setMusicAccess } from "/web/music-access.js";
import { installPalette, paletteKeys } from "/web/palette.js";
import { favouritesPage } from "/web/favourites.js";
import { playerPage } from "/web/player.js";
import { requestsPage } from "/web/requests.js";
import { calendarPage } from "/web/calendar.js";
import { generation, leaving, nextGeneration, setSignIn } from "/web/page-state.js";
import { settingsPage } from "/web/settings.js";
import { placeShell, startShell, stopShell } from "/web/shell.js";
import { tint, titlePage } from "/web/title-page.js";
import { collectionPage, personPage } from "/web/people.js";
import { wantedPage } from "/web/wanted-page.js";

const main = document.getElementById("main");
const bar = document.getElementById("bar");

let me = null;
function leavePage() {
  for (const fn of leaving.splice(0)) fn();
}

// ---------- Sign-in ----------

// "Firefox on Linux": how this browser shows in the user's list of sessions.
function deviceName() {
  const ua = navigator.userAgent;
  const browser = /Edg\//.test(ua)
    ? "Edge"
    : /Firefox\//.test(ua)
      ? "Firefox"
      : /Chrome\//.test(ua)
        ? "Chrome"
        : /Safari\//.test(ua)
          ? "Safari"
          : "Web browser";
  const os = /iPhone|iPad/.test(ua)
    ? "iOS"
    : /Android/.test(ua)
      ? "Android"
      : /Mac OS X/.test(ua)
        ? "macOS"
        : /Windows/.test(ua)
          ? "Windows"
          : /Linux/.test(ua)
            ? "Linux"
            : null;
  return os ? `${browser} on ${os}` : browser;
}

// The pages come back here through page-state.js's signIn.
setSignIn(signIn);

function signIn(message) {
  // Its stream links stop working with the session anyway.
  stopPlayer();
  stopShell();
  me = null;
  // Pages still loading for the signed-out user must not replace the form.
  nextGeneration();
  leavePage();
  bar.hidden = true;
  const username = h("input", { id: "username-field", name: "username", autocomplete: "username", required: true });
  const password = h("input", {
    id: "password-field",
    name: "password",
    type: "password",
    autocomplete: "current-password",
    required: true,
  });
  const error = h("p", { id: "sign-in-error", class: "error", role: "alert" });
  const button = h("button", { type: "submit" }, "Sign in");
  const form = h(
    "form",
    { novalidate: true },
    h("div", {}, h("label", { for: "username-field" }, "Username"), username),
    h("div", {}, h("label", { for: "password-field" }, "Password"), password),
    error,
    button,
  );
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    error.textContent = "";
    if (!username.value.trim() || !password.value) {
      error.textContent = "Enter your username and password.";
      (username.value.trim() ? password : username).focus();
      return;
    }
    button.disabled = true;
    button.textContent = "Signing in…";
    try {
      const answer = await api("/auth/web-login", {
        method: "POST",
        body: { username: username.value.trim(), password: password.value, device_name: deviceName() },
      });
      startSession(answer.user);
    } catch (err) {
      error.textContent = err.status === 401 ? "Wrong username or password." : err.message;
      password.setAttribute("aria-describedby", "sign-in-error");
      password.select();
      password.focus();
      button.disabled = false;
      button.textContent = "Sign in";
    }
  });
  const otherWay = h("div", { class: "other-way" });
  show(
    "Sign in",
    h(
      "div",
      { class: "sign-in" },
      h("h1", {}, "Melete"),
      message ? h("p", { class: "status", role: "status" }, message) : null,
      form,
      otherWay,
    ),
  );
  username.focus();
  providerButton(otherWay, error);
}

// What an identity provider's sign-in came back saying (`?oidc=` on the page's address).
const OIDC_OUTCOMES = {
  denied: "You didn't sign in at the provider.",
  unlinked:
    "That account at the provider isn't linked to anyone here. Sign in with your password, then link it under Settings → Your account.",
  expired: "That sign-in ran out. Try again.",
  browser: "That sign-in was started in another browser. Try again here.",
  failed: "Signing in through the provider didn't work. Try again, or sign in with your password.",
  off: "Signing in through an identity provider isn't set up.",
};

// "Sign in with …" under the password form, when an identity provider is set up.
async function providerButton(box, error) {
  let about;
  try {
    about = await api("/auth/oidc");
  } catch {
    return;
  }
  if (!about.enabled || me) return;
  const button = h("button", { type: "button", class: "quiet" }, `Sign in with ${about.name}`);
  button.addEventListener("click", async () => {
    button.disabled = true;
    error.textContent = "";
    try {
      const started = await api("/auth/oidc/start", { method: "POST", body: { device_name: deviceName(), return_to: null } });
      location.assign(started.authorize_url);
    } catch (err) {
      error.textContent = err.message;
      button.disabled = false;
    }
  });
  box.replaceChildren(h("p", { class: "note" }, "Or"), button);
}

function startSession(user) {
  me = user;
  setMusicAccess(user);
  const name = document.getElementById("username");
  name.textContent = user.username;
  name.title = user.username;
  startShell(user);
  bar.hidden = false;
  openPalette = installPalette(user, document.getElementById("palette-open"));
  startPlayer({ onSignedOut: () => signIn("Your session ended. Sign in again.") });
  startBar();
  render();
}

document.getElementById("sign-out").addEventListener("click", async (event) => {
  const button = event.currentTarget;
  button.disabled = true;
  try {
    await api("/auth/logout", { method: "POST" });
  } catch (err) {
    if (!(err instanceof SignedOut)) {
      button.disabled = false;
      announce(`Couldn't sign out: ${err.message}`);
      return;
    }
  } finally {
    button.disabled = false;
  }
  signIn("Signed out.");
});

let openPalette = null;
paletteKeys(() => (me ? openPalette : null));

// The account menu closes once something in it is chosen, or on a click elsewhere.
const accountMenu = document.querySelector(".account");
accountMenu.addEventListener("click", (event) => {
  if (event.target.closest("a")) accountMenu.open = false;
});
document.addEventListener("click", (event) => {
  if (accountMenu.open && !accountMenu.contains(event.target)) accountMenu.open = false;
});

// ---------- Routing ----------

// "#/series/5?season=2" → { parts: ["series", "5"], params }
function route() {
  const [path, query] = location.hash.replace(/^#\/?/, "").split("?");
  return { parts: path.split("/").filter(Boolean), params: new URLSearchParams(query ?? "") };
}

async function render() {
  if (!me) return;
  const gen = nextGeneration();
  leavePage();
  const { parts, params } = route();
  placeShell({ parts, params });
  const page = () => {
    const [kind, id, under] = parts;
    if (!kind) return home(gen);
    if ((kind === "shows" || kind === "anime" || kind === "movies") && !id) return kindPage(kind, params, gen);
    if (kind === "series" && !id) return oldSeriesList(params);
    if ((kind === "movies" || kind === "series") && /^\d+$/.test(id) && (!under || under === "extras" || under === "cast")) {
      return titlePage(kind, id, under, params, gen, me.is_admin);
    }
    if (kind === "favourites" && !id) return favouritesPage(params, gen);
    if (kind === "people" && /^\d+$/.test(id)) return personPage(id, gen);
    if (kind === "collections" && /^\d+$/.test(id)) return collectionPage(id, gen);
    if (kind === "settings" || kind === "wanted" || kind === "requests") {
      const page = { settings: settingsPage, wanted: wantedPage, requests: requestsPage }[kind];
      return page(id, {
        me,
        current: () => gen === generation,
        onLeave: (fn) => leaving.push(fn),
        signedOut: () => signIn("Your session ended. Sign in again."),
      });
    }
    if (kind === "calendar" && !id) {
      return calendarPage(params, {
        current: () => gen === generation,
        signedOut: () => signIn("Your session ended. Sign in again."),
      });
    }
    if (kind === "admin") {
      return adminPage(parts, {
        me,
        params,
        current: () => gen === generation,
        onLeave: (fn) => leaving.push(fn),
        signedOut: () => signIn("Your session ended. Sign in again."),
      });
    }
    if (kind === "music") {
      const shown = musicPage(parts, params, {
        current: () => gen === generation,
        onLeave: (fn) => leaving.push(fn),
        signedOut: () => signIn("Your session ended. Sign in again."),
        tint,
      });
      if (shown) return shown;
    }
    if (kind === "play" && /^\d+$/.test(id)) {
      // A video and music at once would talk over each other.
      pauseMusic();
      return playerPage(id, params, {
        current: () => gen === generation,
        onLeave: (fn) => leaving.push(fn),
        signedOut: () => signIn("Your session ended. Sign in again."),
      });
    }
    show("Not found", h("h1", {}, "Not found"), h("p", {}, h("a", { href: "#/" }, "Go to the home page")));
  };
  try {
    await page();
  } catch (err) {
    if (gen !== generation) return;
    if (err instanceof SignedOut) return signIn("Your session ended. Sign in again.");
    if (err.status === 404) {
      show("Not found", h("h1", {}, "Not found"), h("p", {}, "Melete has no such title. It may have left the library."));
    } else {
      show("Error", failure(err, () => render()));
    }
  }
  if (gen !== generation) return;
  main.focus({ preventScroll: true });
  // A page reached by Back has its scroll position in its history entry; a new one starts at the
  // top, or where the last one was for links that ask for that.
  window.scrollTo(0, history.state?.scroll ?? keepScroll ?? 0);
  keepScroll = null;
  settled = true;
}

// Each history entry remembers how far its page was scrolled. Only a page that has finished
// loading is saved: the scroll jumps while one is swapped for the next don't belong to either.
let settled = false;
let keepScroll = null;
let saving;
const saveScroll = () => history.replaceState({ scroll: window.scrollY }, "", location.hash);
history.scrollRestoration = "manual";
addEventListener("hashchange", () => {
  clearTimeout(saving);
  settled = false;
  render();
});
addEventListener(
  "scroll",
  () => {
    clearTimeout(saving);
    if (settled) saving = setTimeout(saveScroll, 200);
  },
  { passive: true },
);
// Leaving by a link saves the position at once, not 200 ms later on the next page.
document.addEventListener(
  "click",
  (event) => {
    const link = event.target.closest?.('a[href^="#/"]');
    if (!link || !settled) return;
    clearTimeout(saving);
    saveScroll();
    if (link.hasAttribute("data-keep-scroll")) keepScroll = window.scrollY;
  },
  true,
);

document.querySelector(".skip").addEventListener("click", (event) => {
  // A plain #main link would change the route.
  event.preventDefault();
  main.focus();
});

(async () => {
  // Linking's outcome is the account page's to show; any other is for the sign-in page.
  const outcome = oidcOutcome({ keep: true });
  try {
    const user = await api("/me");
    if (!location.hash.startsWith("#/settings/account")) oidcOutcome();
    startSession(user);
  } catch (err) {
    oidcOutcome();
    if (err instanceof SignedOut) signIn(OIDC_OUTCOMES[outcome] ?? null);
    else show("Error", failure(err, () => location.reload()));
  }
})();
