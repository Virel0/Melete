// What every page of the web UI shares: the API, building DOM nodes, and formatting.

const main = document.getElementById("main");
const announcer = document.getElementById("announce");

// ---------- API ----------

export class SignedOut extends Error {}

// `keepalive` lets a request finish after the page is gone (the last progress report); `signal`
// cancels it (an AbortError is thrown). Paths are under /api/v1, except Aoide's own
// (`/aoide/…`), which sit beside it.
export async function api(path, { method = "GET", body, keepalive = false, signal } = {}) {
  const headers = { "X-Melete-Web": "1" };
  // A file (a Blob) goes as it is, with its own type; anything else as JSON.
  const file = body instanceof Blob;
  if (body !== undefined) headers["Content-Type"] = file ? body.type : "application/json";
  let response;
  try {
    response = await fetch(path.startsWith("/aoide/") ? path : `/api/v1${path}`, {
      method,
      headers,
      body: body === undefined || file ? body : JSON.stringify(body),
      credentials: "same-origin",
      keepalive,
      signal,
    });
  } catch (err) {
    if (err.name === "AbortError") throw err;
    throw new Error("Melete didn't answer. Check that the server is running, then try again.");
  }
  if (response.status === 204) return null;
  const data = await response.json().catch(() => null);
  // Signing in answers 401 for a wrong password or code, and when it needs a code.
  if (response.status === 401 && !path.startsWith("/auth/web-login")) throw new SignedOut();
  if (!response.ok) {
    throw Object.assign(new Error(data?.error ?? `Melete answered ${response.status}.`), {
      status: response.status,
      data,
    });
  }
  return data;
}

// What an identity provider's sign-in came back saying (`?oidc=` in the page's address), taken
// out of the address unless `keep`, so a reload doesn't say it again.
export function oidcOutcome({ keep = false } = {}) {
  const [path, query] = location.hash.split("?");
  const params = new URLSearchParams(query ?? "");
  const outcome = params.get("oidc");
  if (!outcome || keep) return outcome;
  params.delete("oidc");
  const rest = params.toString();
  history.replaceState(history.state, "", `${location.pathname}${path}${rest ? `?${rest}` : ""}`);
  return outcome;
}

// ---------- DOM helpers ----------

// h("a", { href: "#/" }, "Home"): an element with attributes and children (strings become text).
export function h(tag, props = {}, ...children) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (value === null || value === undefined || value === false) continue;
    if (key.startsWith("on")) el.addEventListener(key.slice(2), value);
    else if (key === "class") el.className = value;
    else el.setAttribute(key, value === true ? "" : value);
  }
  append(el, children);
  return el;
}

export function append(el, children) {
  // Nested lists of children (a group given an array, say) flatten all the way down.
  for (const child of children.flat(Infinity)) {
    if (child === null || child === undefined || child === false) continue;
    el.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
}

export function announce(text) {
  announcer.textContent = "";
  // A fresh text node is what screen readers notice.
  setTimeout(() => (announcer.textContent = text), 50);
}

// Puts a page in place: the old one goes, focus moves to it for keyboard and screen readers.
export function show(title, ...children) {
  document.title = title ? `${title} – Melete` : "Melete";
  main.replaceChildren(...children.flat().filter(Boolean));
}

export function loading(what) {
  return h("p", { class: "status" }, `Loading ${what}…`);
}

export function failure(err, retry) {
  return h(
    "div",
    { role: "alert" },
    h("p", { class: "error" }, err.message),
    h("button", { type: "button", class: "quiet", onclick: retry }, "Try again"),
  );
}

// Facts side by side, e.g. year, runtime and genres.
export function metaRow(items) {
  const shown = items.filter(Boolean);
  return shown.length ? h("ul", { class: "meta-row" }, shown.map((item) => h("li", {}, item))) : null;
}

// ---------- Formatting ----------

export const number = new Intl.NumberFormat();

export function plural(n, one, many) {
  return `${number.format(n)} ${n === 1 ? one : many}`;
}

export function size(bytes) {
  const units = ["bytes", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${unit === 0 ? value : value.toFixed(1)} ${units[unit]}`;
}

export function runtime(minutes) {
  if (!minutes) return null;
  const hours = Math.floor(minutes / 60);
  return hours ? `${hours} h ${minutes % 60} min` : `${minutes} min`;
}

// "2024-03-01" as a local date (Date parses that form as UTC, a day early west of London).
export function airDate(text) {
  const [y, m, d] = text.split("-").map(Number);
  return new Date(y, m - 1, d).toLocaleDateString(undefined, { dateStyle: "medium" });
}

export function seasonName(n) {
  return n === 0 ? "Specials" : `Season ${n}`;
}

export function fileName(path) {
  return path.split("/").pop();
}

// 3725 → "1:02:05", 65 → "1:05".
export function clock(seconds) {
  const s = Math.floor(seconds);
  const [hours, minutes, secs] = [Math.floor(s / 3600), Math.floor(s / 60) % 60, s % 60];
  const pad = (n) => String(n).padStart(2, "0");
  return hours ? `${hours}:${pad(minutes)}:${pad(secs)}` : `${minutes}:${pad(secs)}`;
}

// "eng" → "English", as the browser names it; the code itself when it doesn't know it.
export function languageName(code) {
  if (!code) return null;
  try {
    return new Intl.DisplayNames(undefined, { type: "language" }).of(code) ?? code;
  } catch {
    return code;
  }
}

export function episodeLabel(season, episode, name) {
  return `${seasonName(season)}, episode ${episode}${name ? `: ${name}` : ""}`;
}
