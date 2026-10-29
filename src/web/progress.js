// How wanted titles are getting on: a step bar for one title, and the Activity section,
// which follows the server's live progress stream (Server-Sent Events) while it's open.

import { SignedOut, announce, api, h } from "/web/lib.js";
import { group } from "/web/forms.js";

// The road a wanted title takes, in order.
const STEPS = [
  ["searching", "Looking for a download"],
  ["downloading", "Downloading"],
  ["transferring", "Transferring"],
  ["importing", "Importing"],
  ["in_library", "In your library"],
];
// How long to wait before opening the stream again after it drops.
const RETRY_MS = 5000;

// The steps as a list, the current one marked; a failed title says so instead.
export function stepBar(p) {
  const at = STEPS.findIndex(([key]) => key === p.step);
  const steps = h(
    "ol",
    { class: `steps${p.step === "failed" ? " failed" : ""}`, "aria-label": "Progress" },
    STEPS.map(([key, label], i) =>
      h(
        "li",
        {
          class: i < at || p.step === "in_library" ? "done" : i === at ? "current" : null,
          "aria-current": i === at && p.step !== "in_library" ? "step" : null,
        },
        h("span", { class: "label" }, label),
      ),
    ),
  );
  // Phones show this one line instead of the row of steps.
  const summary =
    p.step === "failed"
      ? null
      : p.step === "wanted"
        ? h("p", { class: "summary" }, "Wanted")
        : h("p", { class: "summary" }, `Step ${at + 1} of ${STEPS.length}: ${STEPS[at][1]}`);
  const percent =
    p.step === "downloading" && p.percent !== null
      ? h(
          "p",
          { class: "note" },
          h("progress", { max: 100, value: p.percent, "aria-label": `${p.title}: ${Math.round(p.percent)}% downloaded` }),
          ` ${Math.round(p.percent)}%`,
        )
      : null;
  const said =
    p.step === "failed"
      ? h("p", { class: "error" }, `Failed${p.message ? `: ${p.message}` : "."}`)
      : p.message
        ? h("p", { class: "note" }, sentence(p.message))
        : null;
  return h("div", { class: "progress" }, steps, summary, percent, said);
}

// "in the library; ReplayGain added" → "In the library; ReplayGain added."
function sentence(text) {
  const first = text.charAt(0).toUpperCase() + text.slice(1);
  return /[.!?]$/.test(first) ? first : `${first}.`;
}

const KINDS = { movie: "Movie", series: "Series", album: "Album", song: "Song", artist: "Artist", download: "Download" };

function entry(p) {
  return h(
    "li",
    {},
    h(
      "div",
      {},
      h("strong", {}, p.title),
      " ",
      h("span", { class: "badge" }, KINDS[p.kind] ?? p.kind),
      p.source === "soulseek" ? [" ", h("span", { class: "badge" }, "Soulseek")] : null,
      stepBar(p),
    ),
  );
}

// Who's who between two lists, for saying what just finished.
const key = (p) => `${p.kind}:${p.wanted_id ?? ""}:${p.download_id ?? ""}`;

export async function activity(ctx) {
  // The first answer comes from a plain request, so the section draws at once.
  const first = await api("/progress");
  if (!ctx.current()) return [];
  const list = h("ul", { class: "rows activity" });
  const empty = h("p", { class: "status" }, "Nothing on the move. Searches and downloads show up here as they happen.");
  const live = h("p", { class: "note", role: "status" }, "Updates live while this page is open.");
  let previous = new Map();

  const draw = (items) => {
    const steps = new Map(items.map((p) => [key(p), p.step]));
    // Say only what someone would want to hear about: something arriving or failing.
    for (const p of items) {
      const before = previous.get(key(p));
      if (before && before !== p.step && (p.step === "in_library" || p.step === "failed")) {
        announce(p.step === "failed" ? `${p.title} failed` : `${p.title} is in your library`);
      }
    }
    previous = steps;
    list.replaceChildren(...items.map(entry));
    empty.hidden = items.length > 0;
  };
  draw(first);

  follow(ctx, "/api/v1/progress/events", "progress", draw, live);

  return [group("Activity", live, empty, list)];
}

// Follows a live stream at `path` whose `event` events each carry a whole list, drawing each.
// `live` says when the connection drops; it's opened again, or a signed-out user signs in.
export function follow(ctx, path, event, draw, live) {
  let source = null;
  let retry = null;
  const open = () => {
    if (!ctx.current()) return;
    source = new EventSource(path);
    source.addEventListener(event, (message) => {
      live.textContent = "Updates live while this page is open.";
      try {
        draw(JSON.parse(message.data));
      } catch {
        // A garbled message: the next one replaces it.
      }
    });
    source.addEventListener("error", async () => {
      // The browser retries a dropped stream by itself; one it gave up on is closed.
      if (source.readyState !== EventSource.CLOSED) return;
      live.textContent = "Lost the live connection; trying again…";
      try {
        await api("/me");
      } catch (err) {
        if (err instanceof SignedOut) return ctx.signedOut();
      }
      if (ctx.current()) retry = setTimeout(open, RETRY_MS);
    });
  };
  open();
  ctx.onLeave(() => {
    clearTimeout(retry);
    source?.close();
  });
}
