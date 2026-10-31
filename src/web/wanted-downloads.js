// Downloads, for admins: what was grabbed and how far it has got, removing one (and maybe its
// torrent and files on the seedbox), searching again for a failed one, and testing the seedbox.

import { confirmInline, group, when } from "/web/forms.js";
import { SignedOut, announce, api, h, plural, seasonName, size } from "/web/lib.js";
import { searchResults } from "/web/wanted.js";
import { copyingBar } from "/web/progress.js";

const STATES = {
  downloading: "Downloading",
  completed: "On the seedbox",
  transferring: "Copying",
  imported: "In the library",
  failed: "Failed",
};
// A Soulseek download waits in slskd's folder, not on the seedbox.
const stateName = (d) => (d.source === "soulseek" && d.state === "completed" ? "In slskd's folder" : STATES[d.state] ?? d.state);

// What a download was for, as the admin areas split them.
export const FOR_AREA = {
  movies: (d) => d.wanted_movie_id !== null,
  series: (d) => d.wanted_series_id !== null,
  music: (d) => d.wanted_album_id !== null || d.wanted_song_id !== null,
};

// Which downloads can be removed: a torrent still downloading or waiting on the seedbox,
// or anything that failed. One being copied home or in the library can't be.
const removable = (d) => d.state === "failed" || (d.source !== "soulseek" && ["downloading", "completed"].includes(d.state));

export async function downloads(ctx) {
  // A Manage service may narrow it further (`ctx.downloadsOnly`): Series and Anime apart.
  const mine = (items) => items.filter(ctx.downloadsOnly ?? (ctx.kind ? FOR_AREA[ctx.kind] : () => true));
  const all = mine(await api("/downloads"));
  if (!ctx.current()) return [];
  // What the list's parts share. A row's question or search results outlast the list's
  // redraws every 10 seconds.
  const page = {
    ctx,
    mine,
    list: h("ul", { class: "rows" }),
    asking: new Map(),
    found: new Map(),
    current: all,
    // Clearing every failed download at once, asked first.
    clearBox: h("div", { class: "form-actions" }),
  };
  clearFailed(page);
  draw(page, all);
  // While the page is open, the list follows Deluge's progress.
  const timer = setInterval(async () => {
    try {
      draw(page, mine(await api("/downloads")));
    } catch (err) {
      if (err instanceof SignedOut) {
        clearInterval(timer);
        ctx.signedOut();
      }
    }
  }, 10_000);
  ctx.onLeave(() => clearInterval(timer));

  return [
    group("Downloads", h("p", { class: "note" }, "Newest first; updated every 10 seconds while this page is open. Torrents come through Deluge on the seedbox, Soulseek downloads through slskd."), all.length ? null : h("p", { class: "status" }, "Nothing grabbed yet."), page.clearBox, page.list),
    group("Seedbox", ...seedboxTest(ctx)),
  ];
}

// Asks for the downloads again and redraws the list.
async function refresh(page) {
  draw(page, page.mine(await api("/downloads")));
}

// Draws the list: each download, keeping a row's open question and search results.
function draw(page, items) {
  page.current = items;
  page.list.replaceChildren(...items.map((d) => downloadRow(d, page.asking.get(d.id) ?? rowActions(page, d), page.found.get(d.id) ?? null)));
  page.clearBox.hidden = !items.some((d) => d.state === "failed");
}

// A row's buttons: Search again for a failed one, and Remove for one that can be.
function rowActions(page, d) {
  const actions = h("div", { class: "row-actions" });
  if (d.state === "failed" && (d.wanted_movie_id || (d.wanted_series_id && d.season !== null))) {
    const again = h("button", { type: "button", class: "quiet", "aria-label": `Search again for ${d.title}` }, "Search again");
    again.addEventListener("click", () => searchAgain(page, d, again));
    actions.append(again);
  }
  if (removable(d)) {
    const remove = h("button", { type: "button", class: "quiet", "aria-label": `Remove ${d.title}` }, "Remove");
    remove.addEventListener("click", () => askRemove(page, d, actions));
    actions.append(remove);
  }
  return actions;
}

// One download: its title, state, progress and facts, Melete's word on it, and any results of
// searching again.
function downloadRow(d, actions, found) {
  return h(
    "li",
    {},
    h(
      "div",
      {},
      h("span", { class: "path" }, d.title),
      " ",
      h("span", { class: `badge state-${d.state}` }, stateName(d)),
      d.source === "soulseek" ? [" ", h("span", { class: "badge" }, "Soulseek")] : null,
      d.state === "transferring" && d.copied
        ? copyingBar(d.title, d.copied)
        : d.state === "downloading" || d.state === "transferring"
          ? h("p", {}, h("progress", { max: 100, value: d.progress, "aria-label": `${d.title}: ${Math.round(d.progress)}%` }), ` ${Math.round(d.progress)}%`)
          : null,
      h(
        "p",
        { class: "note" },
        [
          d.season !== null ? (d.episode !== null ? `${seasonName(d.season)}, episode ${d.episode}` : seasonName(d.season)) : null,
          d.size ? size(d.size) : null,
          `added ${when(d.added_at)}`,
          d.completed_at ? `finished ${when(d.completed_at)}` : null,
          d.import_position ? (d.import_position === 1 ? "next to import" : `number ${d.import_position} in line to import`) : null,
        ]
          .filter(Boolean)
          .join(", ") + ".",
      ),
      d.message ? h("p", { class: d.state === "failed" ? "error" : "note" }, d.message) : null,
      found,
    ),
    actions,
  );
}

// Removing asks first, in the row: what happens, and whether Deluge drops the torrent too.
function askRemove(page, d, actions) {
  const { ctx, asking } = page;
  const id = `from-deluge-${d.id}`;
  const deluge = d.source !== "soulseek" && d.state !== "failed";
  const box = h("input", { type: "checkbox", id, checked: deluge });
  const blockId = `blocklist-${d.id}`;
  const blockBox = h("input", { type: "checkbox", id: blockId });
  // Deleting its files on the seedbox: Deluge's own remove-with-data, never the library.
  const dataId = `delete-data-${d.id}`;
  const dataBox = h("input", { type: "checkbox", id: dataId });
  const go = h("button", { type: "button", class: "danger" }, "Remove download");
  const keep = h("button", { type: "button", class: "quiet" }, "Keep it");
  const close = () => {
    asking.delete(d.id);
    draw(page, page.current);
  };
  keep.addEventListener("click", close);
  go.addEventListener("click", async () => {
    go.disabled = keep.disabled = true;
    try {
      if (blockBox.checked) await api("/blocklist", { method: "POST", body: { download_id: d.id } });
      const how = dataBox.checked ? "?delete_data=true" : box.checked ? "?from_deluge=true" : "";
      await api(`/downloads/${d.id}${how}`, { method: "DELETE" });
      asking.delete(d.id);
      announce(`Removed ${d.title}${dataBox.checked ? " and deleted its files on the seedbox" : ""}${blockBox.checked ? "; it's blocklisted" : ""}`);
      await refresh(page);
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      said.textContent = err.message;
      said.classList.add("error");
      go.disabled = keep.disabled = false;
    }
  });
  const stays = `Nothing in a library changes but Melete's own unfinished copy, if one was started, and ${d.wanted_movie_id || d.wanted_series_id || d.wanted_album_id || d.wanted_song_id ? "what it was for stays wanted, so it's looked for again" : "nothing else is affected"}.`;
  const said = h("span", { class: "question" }, `Remove it from this list? ${stays}`);
  const keepsData = "Also remove the torrent from Deluge (what it downloaded stays on the seedbox)";
  const delugeLabel = h("label", { for: id }, keepsData);
  dataBox.addEventListener("change", () => {
    // Deleting the files takes the torrent out of Deluge with them.
    if (dataBox.checked) box.checked = true;
    box.disabled = dataBox.checked;
    said.textContent = dataBox.checked
      ? `Remove ${d.title} and delete its files on the seedbox? They're deleted for good. ${stays}`
      : `Remove it from this list? ${stays}`;
    go.textContent = dataBox.checked ? "Remove and delete files" : "Remove download";
    delugeLabel.textContent = dataBox.checked ? "Also remove the torrent from Deluge" : keepsData;
  });
  // A Soulseek download has no torrent nor seedbox files to remove.
  const seedbox =
    d.source === "soulseek"
      ? []
      : [
          h("div", { class: "check" }, box, delugeLabel),
          h("div", { class: "check" }, dataBox, h("label", { for: dataId }, "Also delete its files on the seedbox, for good")),
        ];
  actions.replaceChildren(
    said,
    ...seedbox,
    h("div", { class: "check" }, blockBox, h("label", { for: blockId }, "Blocklist this release, so it's never grabbed again")),
    go,
    keep,
  );
  asking.set(d.id, actions);
  keep.focus();
}

// Searching again for what a failed download was for, with the results in its row.
async function searchAgain(page, d, button) {
  const { ctx, found } = page;
  const kind = d.wanted_movie_id ? "movies" : "series";
  const wantedId = d.wanted_movie_id ?? d.wanted_series_id;
  const body = d.wanted_series_id ? { season: d.season, episode: d.episode } : undefined;
  button.disabled = true;
  found.set(d.id, h("p", { class: "status" }, "Asking the indexers…"));
  draw(page, page.current);
  try {
    const search = await api(`/wanted/${kind}/${wantedId}/search`, { method: "POST", body });
    found.set(d.id, searchResults(search, ctx));
  } catch (err) {
    if (err instanceof SignedOut) return ctx.signedOut();
    found.set(d.id, h("p", { class: "error" }, err.message));
  }
  draw(page, page.current);
}

// Clear failed (N), which asks first; hidden while nothing has failed.
function clearFailed(page) {
  const { ctx, clearBox } = page;
  const failed = page.current.filter((d) => d.state === "failed").length;
  const clear = h("button", { type: "button", class: "quiet" }, `Clear failed (${failed})`);
  clear.addEventListener("click", () =>
    confirmInline(clearBox, {
      question: `Remove ${plural(failed, "failed download", "failed downloads")} from the list? What they were for stays wanted; any unfinished copies Melete started are deleted.`,
      confirm: "Clear failed",
      keep: "Keep them",
      run: async () => {
        const r = await api(`/downloads/clear-failed${ctx.kind ? `?for=${ctx.kind}` : ""}`, { method: "POST" });
        announce(`Removed ${plural(r.removed, "failed download", "failed downloads")}`);
        clearBox.replaceChildren(h("p", { class: "form-result", role: "status" }, `Removed ${plural(r.removed, "failed download", "failed downloads")}.`));
        await refresh(page);
      },
    }),
  );
  clearBox.replaceChildren(clear);
}

// Testing the seedbox connection: the button, and where the answer goes.
function seedboxTest(ctx) {
  const testResult = h("div", { role: "status" });
  const test = h("button", { type: "button", class: "quiet" }, "Test the seedbox connection");
  test.addEventListener("click", async () => {
    test.disabled = true;
    testResult.replaceChildren(h("p", { class: "status" }, "Connecting…"));
    try {
      const t = await api("/seedbox/test", { method: "POST" });
      testResult.replaceChildren(
        h("p", {}, `Connected; the host key matched (${t.host_key}). ${t.folder} has ${plural(t.entries, "entry", "entries")}.`),
        h(
          "ul",
          { class: "rows" },
          t.completed.map((c) =>
            h("li", {}, h("div", {}, h("span", { class: "path" }, c.deluge_path), h("p", { class: c.found ? "note" : "error" }, c.sftp_path ? `${c.sftp_path}: ${c.found ? "found" : "not found"}` : "Outside the folder for finished torrents."))),
          ),
        ),
      );
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      testResult.replaceChildren(h("p", { class: "error" }, err.message));
    } finally {
      test.disabled = false;
    }
  });
  return [h("p", { class: "note" }, "Connects over SFTP only if the seedbox shows the pinned host key, then checks the folders downloads are copied from."), h("div", { class: "form-actions" }, test), testResult];
}
