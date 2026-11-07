// Hidden songs: the songs this listener flagged on any device or here, from the same flags their
// Aoide devices keep (`GET /music/flags`): "Not interested" ones are kept out of mixes, radio,
// shuffle and smart playlists, and "Don't count plays" ones have no new listens kept. Each flag
// can be cleared here, and a list's flags all at once.
//
// Flags live in Aoide's sync log, which is closed to someone who may see only some music
// libraries: that refusal (403) is the page's one sentence.

import { confirmInline } from "/web/forms.js";
import { SignedOut, announce, api, h, plural, show } from "/web/lib.js";
import { saidOf, setFlag } from "/web/music-flags.js";
import { goneRow } from "/web/music-liked.js";
import { SYNC_CLOSED, seesAllMusic } from "/web/music-access.js";
import { emptyState, failed, hold, placeholders, release } from "/web/page-parts.js";

const LISTS = [
  ["not_interested", "Not interested", "Kept out of mixes, radio, shuffle and smart playlists.", "Show in mixes again"],
  ["dont_count", "Plays not counted", "New listens of these aren't kept.", "Count plays again"],
];

export async function hiddenPage(ctx) {
  const title = h("h1", {}, "Hidden songs");
  if (!seesAllMusic()) return show("Hidden songs", title, emptyState(SYNC_CLOSED));
  show("Hidden songs", title, placeholders("your hidden songs"));
  let flags;
  try {
    flags = await api("/music/flags");
  } catch (err) {
    if (err instanceof SignedOut) throw err;
    if (!ctx.current()) return;
    if (err.status === 403) return show("Hidden songs", title, emptyState(SYNC_CLOSED));
    return show("Hidden songs", title, failed("your hidden songs", err, () => hiddenPage(ctx)));
  }
  if (!ctx.current()) return;
  if (!flags.items.length) {
    show(
      "Hidden songs",
      title,
      emptyState("Songs you mark Not interested or Don't count plays show here. Choose More beside any song to mark one."),
    );
    return;
  }
  show(
    "Hidden songs",
    title,
    h("p", { class: "lead" }, "Songs you marked on any of your devices or here. Clearing a mark here clears it everywhere."),
    LISTS.map(([key, heading, about, clear]) => section(flags.items.filter((i) => i[key]), key, heading, about, clear, ctx)),
  );
}

function section(items, key, heading, about, clear, ctx) {
  if (!items.length) return null;
  const id = `hidden-${key}`;
  const list = h("ul", { class: "tracks songs hidden-list" });
  const said = h("p", { class: "status", role: "status" }, plural(items.length, "song", "songs"));
  const rows = items.map((item) => row(item, key, clear, list, said));
  list.append(...rows);
  const tools = h("div", { class: "row-actions" });
  const present = items.filter((i) => i.track);
  if (present.length > 1) {
    const all = h("button", { type: "button", class: "quiet" }, `${clear}: all ${present.length}`);
    all.addEventListener("click", () =>
      confirmInline(tools, {
        question: `${clear} for all ${plural(present.length, "song", "songs")} in this list?`,
        confirm: clear,
        run: async () => {
          for (const item of present) await setFlag(item.track, key, false);
          announce(`${plural(present.length, "song", "songs")} changed.`);
          await hiddenPage(ctx);
        },
      }),
    );
    tools.append(all);
  }
  return h("section", { class: "hidden-group", "aria-labelledby": id }, h("h2", { id, tabindex: "-1" }, heading), h("p", { class: "lead" }, about), said, list, tools);
}

function row(item, key, clear, list, said) {
  const track = item.track;
  if (!track) return goneRow(item.content_key);
  const button = h("button", { type: "button", class: "quiet small", "aria-label": `${clear}: ${track.title}` }, clear);
  button.addEventListener("click", async () => {
    if (!hold(button)) return;
    try {
      await setFlag(track, key, false);
      const at = [...list.children].indexOf(li);
      li.remove();
      const left = list.children.length;
      said.textContent = `${saidOf(key, false, track.title)} ${left ? `${plural(left, "song", "songs")} left.` : "None left."}`;
      const next = (list.children[at] ?? list.children[at - 1])?.querySelector("button");
      if (next) next.focus();
      else list.closest("section")?.querySelector("h2")?.focus();
    } catch (err) {
      if (err instanceof SignedOut) throw err;
      said.textContent = `Couldn't change ${track.title}: ${err.message}`;
      release(button);
    }
  });
  const li = h(
    "li",
    {},
    h("span", { class: "name" }, h("span", {}, track.title), h("span", { class: "by" }, track.artist ?? track.album_artist)),
    h("a", { class: "album clip", href: `#/music/albums/${track.album_id}`, title: track.album }, track.album),
    button,
  );
  return li;
}
