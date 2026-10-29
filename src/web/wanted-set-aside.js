// What was set aside, each with a way back: releases on the blocklist, titles taken off the
// wanted list, and files an upgrade replaced, in the trash.

import { confirmInline, group, when } from "/web/forms.js";
import { SignedOut, api, h, plural, size } from "/web/lib.js";

// ---------- The blocklist ----------

// Releases never to be grabbed again, each with a way to take it off.
export async function blocklist(ctx) {
  const all = await api("/blocklist");
  if (!ctx.current()) return [];
  const rows = all.map((b) => {
    const actions = h("div", { class: "row-actions" });
    const off = h("button", { type: "button", class: "quiet", "aria-label": `Take ${b.title} off the blocklist` }, "Take off");
    off.addEventListener("click", () =>
      confirmInline(actions, {
        question: `Take ${b.title} off the blocklist? Searches and syncs can grab it again.`,
        confirm: "Take off the blocklist",
        keep: "Keep it blocked",
        run: async () => {
          await api(`/blocklist/${b.id}`, { method: "DELETE" });
          ctx.reload(`${b.title} is off the blocklist.`);
        },
      }),
    );
    actions.append(off);
    return h(
      "li",
      {},
      h(
        "div",
        {},
        h("span", { class: "path" }, b.title),
        b.source === "soulseek" ? [" ", h("span", { class: "badge" }, "Soulseek")] : null,
        h("p", { class: "note" }, `Blocklisted ${when(b.added_at)}.`),
      ),
      actions,
    );
  });
  return [
    group(
      "Blocklist",
      h(
        "p",
        { class: "note" },
        "These are never grabbed, by hand or by a sync; a sync takes the next best result instead. A release is known by its name, and a torrent also by its contents, so a renamed copy is caught too. Blocklist a release from a search result, or when removing its download.",
      ),
      rows.length ? h("ul", { class: "rows" }, rows) : h("p", { class: "status" }, "Nothing blocklisted."),
    ),
  ];
}

// ---------- Titles taken off the wanted list ----------

const TAKEN_OFF = {
  movies: { kinds: ["movie"], from: "Importing from Radarr and import lists leave these off." },
  series: { kinds: ["series"], from: "Importing from Sonarr and import lists leave these off." },
  music: { kinds: ["artist", "album"], from: "Importing from Lidarr leaves these off." },
};

// What an admin took off the wanted list in an area, with when and by whom: allowed
// back, the next import or list sync may add it; added again now, it's wanted as an approved
// request would be.
export async function removedTitles(ctx, area) {
  const { kinds, from } = TAKEN_OFF[area];
  const all = (await api("/wanted/removed")).filter((r) => kinds.includes(r.kind));
  if (!ctx.current()) return [];
  const music = area === "music";
  const row = (r) => {
    const said = h("p", { class: "form-result", role: "status" });
    const act = (button, path, done) => {
      button.addEventListener("click", async () => {
        for (const b of actions.querySelectorAll("button")) b.disabled = true;
        said.textContent = "";
        try {
          await api(`/wanted/removed/${path}`, { method: "POST", body: { kind: r.kind, key: r.key } });
          ctx.reload(done);
        } catch (err) {
          if (err instanceof SignedOut) return ctx.signedOut();
          said.textContent = err.message;
          said.classList.add("error");
          for (const b of actions.querySelectorAll("button")) b.disabled = false;
          button.focus();
        }
      });
      return button;
    };
    const actions = h("div", { class: "row-actions" });
    actions.append(
      act(h("button", { type: "button", class: "quiet", "aria-label": `Add ${r.title} to the wanted list again now` }, "Add it again now"), "add", `“${r.title}” is on the wanted list again.`),
      act(h("button", { type: "button", class: "quiet", "aria-label": `Allow ${r.title} back` }, "Allow it back"), "allow", `“${r.title}” can come back with the next ${music ? "import" : "import or list sync"}.`),
    );
    return h(
      "li",
      {},
      h(
        "div",
        {},
        h("strong", {}, r.title),
        music ? [" ", h("span", { class: "badge" }, r.kind === "artist" ? "Artist" : "Album")] : null,
        h("p", { class: "note" }, `Taken off ${when(r.removed_at)}${r.removed_by ? ` by ${r.removed_by}` : ""}.`),
        said,
      ),
      actions,
    );
  };
  return [
    group(
      "Taken off the wanted list",
      h(
        "p",
        { class: "note" },
        `${from} Allow one back to let the next ${music ? "import" : "import or list sync"} add it again, or add it again now${music ? ": an album comes back with its artist, an artist whole with every album" : ""}. Adding a title by hand or approving a request for it also brings it back.`,
      ),
      all.length
        ? h("ul", { class: "rows" }, all.map(row))
        : h("p", { class: "status" }, "Nothing taken off the list. A title you take off the wanted list shows here, so you can let it back."),
    ),
  ];
}

// ---------- Trash ----------

// `ctx.trashOnly(library)`, when given, narrows it to a Manage service's libraries, as its
// Replaced files; emptying it all stays System's.
export async function trash(ctx) {
  const [every, libraries] = await Promise.all([api("/trash"), api("/libraries")]);
  if (!ctx.current()) return [];
  const only = ctx.trashOnly;
  const all = only ? every.filter((t) => only(libraries.find((l) => l.id === t.library_id))) : every;
  const library = (id) => libraries.find((l) => l.id === id)?.name ?? `library ${id}`;
  const rows = all.map((t) => {
    const actions = h("div", { class: "row-actions" });
    const back = h("button", { type: "button", class: "quiet" }, "Put back");
    back.addEventListener("click", () =>
      confirmInline(actions, {
        question: `Put it back at ${t.rel_path}? The file that replaced it stays where it is.`,
        confirm: "Put it back",
        run: async () => {
          await api(`/trash/${t.id}/restore`, { method: "POST" });
          ctx.reload(`Put back ${t.rel_path}. A rescan follows.`);
        },
      }),
    );
    const now = h("button", { type: "button", class: "quiet" }, "Delete now");
    now.addEventListener("click", () =>
      confirmInline(actions, {
        question: `Delete ${t.rel_path} (${size(t.size)}) for good? It can't be put back afterwards.`,
        confirm: "Delete it now",
        run: async () => {
          await api(`/trash/${t.id}`, { method: "DELETE" });
          ctx.reload(`Deleted ${t.rel_path} from the trash.`);
        },
      }),
    );
    actions.append(back, now);
    const days = Math.max(0, Math.ceil((t.delete_after - Date.now() / 1000) / 86400));
    return h(
      "li",
      {},
      h(
        "div",
        {},
        h("span", { class: "path" }, t.rel_path),
        h("p", { class: "note" }, `${library(t.library_id)}, ${size(t.size)}. Replaced by ${t.replaced_by} on ${when(t.trashed_at)}.`),
        h("p", { class: "note" }, days ? `Deleted in ${plural(days, "day", "days")}.` : "Deleted at the next clean-up."),
      ),
      actions,
    );
  });
  // Emptying it all at once: says how many files and how much space, and can't be undone.
  const emptyActions = h("div", { class: "row-actions" });
  if (all.length && !only) {
    const total = all.reduce((sum, t) => sum + t.size, 0);
    const empty = h("button", { type: "button", class: "quiet" }, "Empty the trash");
    empty.addEventListener("click", () =>
      confirmInline(emptyActions, {
        question: `Delete all ${plural(all.length, "file", "files")} in the trash (${size(total)}) for good? None of them can be put back afterwards.`,
        confirm: "Empty the trash",
        run: async () => {
          const done = await api("/trash", { method: "DELETE" });
          const left = done.left ? ` ${plural(done.left, "file was", "files were")} left: not what the trash put there, or its library is away.` : "";
          ctx.reload(`Deleted ${plural(done.deleted, "file", "files")} from the trash.${left}`);
        },
      }),
    );
    emptyActions.append(empty);
  }
  return [
    group(
      only ? "Replaced files" : "Trash",
      h("p", { class: "note" }, `Files an upgrade replaced, kept in each library's .melete-trash folder for 30 days once the upgrade is in place, then deleted. Delete one now${only ? "" : ", or empty the trash,"} to free the space sooner.`),
      all.length && !only ? emptyActions : null,
      all.length ? h("ul", { class: "rows" }, rows) : h("p", { class: "status" }, "Nothing in the trash."),
    ),
  ];
}
