// Requests: anyone signed in finds a movie, series, artist, album or song and asks for
// it, then follows their requests live; admins decide on everyone's, and set where approved
// ones go and whose requests go straight through.

import { field, form, group, sectioned } from "/web/forms.js";
import { SignedOut, announce, api, h, plural } from "/web/lib.js";
import { follow } from "/web/progress.js";
import { decide, settings } from "/web/requests-admin.js";
import { imports } from "/web/requests-imports.js";
import { KINDS, facts, requestRow } from "/web/requests-row.js";

export { decide, settings } from "/web/requests-admin.js";
export { imports } from "/web/requests-imports.js";

const SECTIONS = [
  { key: "ask", name: "Ask for something", page: ask },
  { key: "mine", name: "Your requests", page: mine },
  { key: "imports", name: "Playlist imports", page: imports },
  { key: "decide", name: "To decide", admin: true, page: decide },
  { key: "settings", name: "Where they go", admin: true, page: settings },
];

export function requestsPage(section, ctx) {
  return sectioned({ title: "Requests", base: "#/requests", sections: SECTIONS }, section, ctx);
}

// What asking said, in words for the one who asked.
function outcome(asked) {
  if (asked.state === "approved") return `Approved: ${asked.title} is on its way.`;
  return `Asked for ${asked.title}. An admin will decide.`;
}

// ---------- Ask for something ----------

async function ask(ctx) {
  const kind = field(
    "ask-kind",
    "What",
    h("select", {}, Object.entries(KINDS).map(([key, name]) => h("option", { value: key }, key === "artist" ? "Artist (everything they release)" : name))),
  );
  const q = field("ask-q", "Title", h("input", { type: "search", autocomplete: "off", maxlength: 200 }));
  const by = field("ask-by", "Artist (optional)", h("input", { autocomplete: "off", maxlength: 200 }), "Narrows the search to one artist.");
  const results = h("div", { class: "results", "aria-live": "polite" });
  const byName = songByName(ctx);

  const shape = () => {
    const k = kind.input.value;
    q.box.querySelector("label").textContent = k === "artist" ? "Name" : "Title";
    by.box.hidden = !(k === "album" || k === "song");
    byName.hidden = k !== "song";
  };
  kind.input.addEventListener("change", () => {
    shape();
    results.replaceChildren();
  });
  shape();

  const search = form([kind, q, by], "Search", async () => {
    const words = q.input.value.trim();
    if (!words) {
      q.fail(kind.input.value === "artist" ? "Enter the artist's name." : "Enter a title to look for.");
      return null;
    }
    const params = new URLSearchParams({ kind: kind.input.value, q: words });
    if (!by.box.hidden && by.input.value.trim()) params.set("artist", by.input.value.trim());
    results.replaceChildren(h("p", { class: "status" }, "Looking…"));
    let found;
    try {
      found = await api(`/requests/search?${params}`);
    } catch (err) {
      results.replaceChildren();
      throw err;
    }
    if (!ctx.current()) return null;
    results.replaceChildren(
      found.length
        ? h("ul", { class: "rows" }, found.map((c) => candidateRow(ctx, c)))
        : h("p", { class: "status" }, "Nothing found. Try fewer words, or another spelling."),
    );
    return found.length ? `${plural(found.length, "result", "results")}.` : null;
  });

  return [
    group(
      "Ask for something",
      h("p", { class: "note" }, "Find it, then ask for it. An admin decides, unless your requests go straight through; follow them under Your requests."),
      search,
      results,
    ),
    byName,
  ];
}

// One thing found, with a button to ask for it, or why there's no need.
function candidateRow(ctx, c) {
  const result = h("span", { class: "note", role: "status" });
  const actions = h("div", { class: "row-actions" });
  const badges = [
    c.in_library ? h("span", { class: "badge" }, "In your library") : null,
    c.wanted && !c.in_library ? h("span", { class: "badge" }, "Wanted") : null,
  ];
  if (c.requested) {
    actions.append(h("a", { class: "badge", href: "#/requests/mine" }, "You asked for this"));
  } else if (c.in_library && (c.kind === "movie" || c.kind === "album" || c.kind === "song")) {
    // Nothing to ask for: it's here.
  } else {
    const id = `season-${c.kind}-${c.tmdb_id ?? c.mb_artist_id ?? c.mb_release_group_id ?? c.mb_recording_id}`;
    const season =
      c.kind === "series"
        ? h("input", { id, inputmode: "numeric", autocomplete: "off", class: "year" })
        : null;
    const button = h("button", { type: "button" }, "Ask for it");
    button.addEventListener("click", async () => {
      // Each kind is asked for by its own id.
      const ids = { movie: "tmdb_id", series: "tmdb_id", artist: "mb_artist_id", album: "mb_release_group_id", song: "mb_recording_id" };
      const body = { kind: c.kind, [ids[c.kind]]: c[ids[c.kind]] };
      const wanted = season?.value.trim();
      if (wanted) {
        if (!/^\d+$/.test(wanted)) {
          result.textContent = "Enter a season number, or leave it empty for the whole series.";
          season.setAttribute("aria-invalid", "true");
          season.focus();
          return;
        }
        body.season = Number(wanted);
      }
      season?.removeAttribute("aria-invalid");
      button.disabled = true;
      result.textContent = "Asking…";
      try {
        const asked = await api("/requests", { method: "POST", body });
        result.textContent = outcome(asked);
        announce(outcome(asked));
        actions.replaceChildren(h("a", { class: "badge", href: "#/requests/mine" }, "See your requests"), result);
      } catch (err) {
        if (err instanceof SignedOut) return ctx.signedOut();
        result.textContent = err.message;
        button.disabled = false;
      }
    });
    if (season) actions.append(h("label", { for: id }, "Season (empty for all)"), season);
    actions.append(button, result);
  }
  return h(
    "li",
    {},
    h(
      "div",
      {},
      h("strong", {}, c.title),
      " ",
      badges,
      facts(c),
      // "Album" again says nothing; "Live Album" does.
      c.detail && c.detail !== KINDS[c.kind] ? h("p", { class: "note" }, c.detail) : null,
    ),
    actions,
  );
}

// A song MusicBrainz doesn't know, asked for by its title and artist.
function songByName(ctx) {
  const title = field("ask-song-title", "Song title", h("input", { autocomplete: "off", maxlength: 300 }));
  const artist = field("ask-song-artist", "Artist", h("input", { autocomplete: "off", maxlength: 300 }));
  const asking = form([title, artist], "Ask for the song", async () => {
    const body = { kind: "song", title: title.input.value.trim(), artist: artist.input.value.trim() };
    if (!body.title) title.fail("Enter the song's title.");
    if (!body.artist) artist.fail("Enter who it's by.");
    if (!body.title || !body.artist) return null;
    const asked = await api("/requests", { method: "POST", body });
    if (!ctx.current()) return null;
    announce(outcome(asked));
    return outcome(asked);
  });
  return group("Can't find the song?", h("p", { class: "note" }, "Ask for it by its title and who it's by."), asking);
}

// ---------- Your requests ----------

async function mine(ctx) {
  const first = await api("/requests?limit=50");
  if (!ctx.current()) return [];
  const list = h("ul", { class: "rows" });
  const empty = h(
    "p",
    { class: "status" },
    "You haven't asked for anything yet. ",
    h("a", { href: "#/requests/ask" }, "Ask for something"),
    ".",
  );
  const live = h("p", { class: "note", role: "status" }, "Updates live while this page is open.");
  let previous = new Map();
  // Which "Other seasons" are open, kept across live redraws.
  const open = new Set();
  const draw = (items) => {
    const now = new Map(items.map((r) => [r.id, `${r.state}:${r.progress?.step ?? ""}`]));
    // Said aloud: a decision, and something arriving or failing.
    for (const r of items) {
      const before = previous.get(r.id);
      if (!before || before === now.get(r.id)) continue;
      const step = r.progress?.step;
      if (r.state === "declined") announce(`Your request for ${r.title} was declined`);
      else if (step === "in_library") announce(`${r.title} is in your library`);
      else if (step === "failed") announce(`${r.title} failed`);
      else if (before.startsWith("requested") && r.state === "approved") announce(`Your request for ${r.title} was approved`);
    }
    previous = now;
    // Other seasons are offered once per series, on its newest request (the list is newest
    // first).
    const offered = new Set();
    list.replaceChildren(
      ...items.map((r) => {
        const others = r.kind === "series" && !offered.has(r.tmdb_id);
        if (others) offered.add(r.tmdb_id);
        return requestRow(ctx, r, { items, others, open });
      }),
    );
    empty.hidden = items.length > 0;
  };
  draw(first.items);
  follow(ctx, "/api/v1/requests/events", "requests", draw, live);
  return [group("Your requests", live, empty, list)];
}
