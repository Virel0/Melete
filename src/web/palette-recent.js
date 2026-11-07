// The palette's recent searches, kept by this browser alone: what was typed before a result was
// chosen, newest first, a few of them, each once. The palette offers them while its box is empty,
// with Clear recent searches after them.

const KEY = "melete.recent-searches";
const KEEP = 6;

// What this browser keeps; none when it keeps nothing, or storage is turned off.
export function recentSearches() {
  try {
    const kept = JSON.parse(localStorage.getItem(KEY) ?? "[]");
    return Array.isArray(kept) ? kept.filter((q) => typeof q === "string" && q.trim()).slice(0, KEEP) : [];
  } catch {
    return [];
  }
}

// Keeps `typed` first, once, case aside.
export function rememberSearch(typed) {
  const q = typed.trim();
  if (q.length < 2) return;
  const rest = recentSearches().filter((r) => r.toLowerCase() !== q.toLowerCase());
  try {
    localStorage.setItem(KEY, JSON.stringify([q, ...rest].slice(0, KEEP)));
  } catch {
    // Storage turned off: nothing is kept.
  }
}

export function clearSearches() {
  try {
    localStorage.removeItem(KEY);
  } catch {
    // Nothing was kept.
  }
}
