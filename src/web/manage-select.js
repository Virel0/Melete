// The bar under a Manage library's toolbar while selecting: how many are selected, Select all
// shown (every page the filters show, not just what's drawn) and Clear; then Monitor, Stop
// monitoring, Set profile and Take off the list (asked first, saying files stay). Monitor also
// puts titles only in the library on the wanted list, monitored, with the profile chosen here
// (or as Where they go says), without searching. Changes go in batches, one request each, and
// the result says how many changed and why any didn't.

import { confirmInline } from "/web/forms.js";
import { SignedOut, announce, api, h, number, plural } from "/web/lib.js";

// Titles a request changes at most: the server takes 100; a title only in the library is looked
// up on TMDB or MusicBrainz first, so fewer of those go at once.
const BATCH = 100;
const BATCH_WANTING = 20;
// Why a title wasn't changed, named, at most; the rest are counted.
const NAMED = 5;

// A title's folder in the library: its movie's, series' or artist's.
export const folderOf = (t) => t.movie_id ?? t.series_id ?? t.artist_id ?? null;

// The key a title is selected by.
export const selectKey = (t) => (t.wanted_id !== null ? `w${t.wanted_id}` : `f${folderOf(t)}`);

// Requests of at most `size` ids each, wanted ids first.
function batches(wanted, folders) {
  const out = [];
  for (let i = 0; i < wanted.length; i += BATCH) out.push({ wanted_ids: wanted.slice(i, i + BATCH), folders: [] });
  for (let i = 0; i < folders.length; i += BATCH_WANTING) out.push({ wanted_ids: [], folders: folders.slice(i, i + BATCH_WANTING) });
  return out;
}

// `selected()` gives the chosen rows; `all()` selects every title shown and resolves when it
// has; `clear()` drops the selection; `done(message)` draws the list again.
export function selectionBar(ctx, kind, { selected, profiles, done, all, clear, shown }) {
  const count = h("p", { class: "selected-count", role: "status" });
  const hint = h("p", { class: "note selection-hint", hidden: true });
  const everything = h("button", { type: "button", class: "quiet" });
  const none = h("button", { type: "button", class: "quiet" }, "Clear");
  const actions = h("div", { class: "row-actions" });
  const busy = (on) => {
    for (const b of actions.querySelectorAll("button, select")) b.disabled = on;
    everything.disabled = none.disabled = on;
  };
  everything.addEventListener("click", async () => {
    busy(true);
    count.textContent = `Selecting all ${number.format(shown())}…`;
    try {
      await all();
      announce(count.textContent);
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      count.textContent = `Couldn't select them all: ${err.message}`;
      busy(false);
    }
  });
  none.addEventListener("click", () => {
    clear();
    announce("Selection cleared");
  });
  // Runs `action` over the selection in batches; `said(n)` words what was done to n titles.
  const run = async (action, said, { profile = null } = {}) => {
    const titles = selected();
    const onList = titles.filter((t) => t.wanted_id !== null);
    const offList = action === "monitor" ? titles.filter((t) => t.wanted_id === null && folderOf(t) !== null) : [];
    const skipped = titles.length - onList.length - offList.length;
    if (!onList.length && !offList.length) {
      count.textContent = "None of them is on the wanted list, so there's nothing to change.";
      return;
    }
    busy(true);
    const named = new Map(titles.map((t) => [selectKey(t), t.title]));
    let changed = 0;
    let added = 0;
    let sent = 0;
    const why = [];
    const total = onList.length + offList.length;
    try {
      for (const batch of batches(onList.map((t) => t.wanted_id), offList.map(folderOf))) {
        count.textContent = `Changing ${plural(total, kind.one, kind.many)}: ${number.format(sent)} done…`;
        const r = await api(`${kind.list}/selection`, { method: "POST", body: { action, profile_id: profile, ...batch } });
        changed += r.changed;
        added += r.wanted.length;
        sent += batch.wanted_ids.length + batch.folders.length;
        for (const u of r.unchanged) why.push(`${named.get(u.wanted_id !== null ? `w${u.wanted_id}` : `f${u.folder}`) ?? "One"}: ${u.why}`);
      }
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      why.push(`the rest weren't sent: ${err.message}`);
    }
    const parts = [changed ? said(plural(changed, kind.one, kind.many)) : "Nothing changed."];
    if (added) parts.push(`${plural(added, "was", "were")} put on the wanted list; Search all missing… looks for what they lack.`);
    if (skipped) parts.push(`${plural(skipped, `${kind.one} isn't`, `${kind.many} aren't`)} on the wanted list, so left as ${skipped === 1 ? "it is" : "they are"}.`);
    if (why.length) parts.push(`Not changed: ${why.slice(0, NAMED).join("; ")}${why.length > NAMED ? `; and ${number.format(why.length - NAMED)} more` : ""}.`);
    const message = parts.join(" ");
    announce(message);
    done(message);
  };
  const profile = h("select", { class: "compact", "aria-label": "Profile" }, h("option", { value: "" }, "Profile…"), profiles.map((p) => h("option", { value: String(p.id) }, p.name)));
  const setProfile = h("button", { type: "button", class: "quiet" }, "Set profile");
  setProfile.addEventListener("click", () => {
    if (!profile.value) return profile.focus();
    const name = profile.selectedOptions[0].textContent;
    run("profile", (n) => `Set ${name} on ${n}.`, { profile: Number(profile.value) });
  });
  const takeOff = h("button", { type: "button", class: "quiet" }, "Take off the list…");
  takeOff.addEventListener("click", () => {
    const n = selected().filter((t) => t.wanted_id !== null).length;
    if (!n) return run("remove", () => "");
    const them = n === 1 ? ["it", "its"] : ["them", "their"];
    confirmInline(actions, {
      question: `Take ${plural(n, kind.one, kind.many)} off the wanted list? Melete stops looking for ${them[0]}; ${them[1]} files in the library stay, and ${them[0]} can be added again.`,
      confirm: "Take off",
      run: () => run("remove", (n) => `Took ${n} off the wanted list.`),
    });
  });
  const monitor = h("button", { type: "button", class: "quiet" }, "Monitor");
  monitor.addEventListener("click", () => run("monitor", (n) => `Monitoring ${n}.`, { profile: profile.value ? Number(profile.value) : null }));
  const parts = [
    monitor,
    h("button", { type: "button", class: "quiet", onclick: () => run("unmonitor", (n) => `Stopped monitoring ${n}.`) }, "Stop monitoring"),
    profiles.length ? [profile, setProfile] : null,
    takeOff,
  ];
  const bar = h("div", { class: "selection-bar" }, count, h("div", { class: "row-actions" }, everything, none), actions, hint);
  return {
    bar,
    update() {
      const chosen = selected();
      const n = chosen.length;
      const off = chosen.filter((t) => t.wanted_id === null).length;
      count.textContent = n ? `${plural(n, kind.one, kind.many)} selected` : `Select ${kind.many} to change them.`;
      everything.textContent = `Select all ${number.format(shown())} shown`;
      if (!actions.querySelector(".question")) actions.replaceChildren(...parts.flat().filter(Boolean));
      busy(false);
      for (const b of actions.querySelectorAll("button, select")) b.disabled = n === 0;
      everything.disabled = shown() === 0 || n >= shown();
      none.disabled = n === 0;
      hint.hidden = off === 0;
      hint.textContent = off
        ? `${plural(off, `${kind.one} isn't`, `${kind.many} aren't`)} on the wanted list. Monitor puts ${off === 1 ? "it" : "them"} there, monitored, in ${off === 1 ? "its" : "their"} own library, with ${profiles.length ? "the profile chosen here (or the one Where they go says)" : "the profile Where they go says"}, without searching; the other actions leave ${off === 1 ? "it" : "them"} as ${off === 1 ? "it is" : "they are"}.`
        : "";
    },
  };
}
