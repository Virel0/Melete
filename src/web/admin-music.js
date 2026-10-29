// Manage → Music: following artists for new albums, matching albums MusicBrainz couldn't
// settle by hand, and music quality profiles and imports from Lidarr.

import { confirmInline, field, form, group, when } from "/web/forms.js";
import { SignedOut, announce, api, h, plural } from "/web/lib.js";
import { arrBody, arrFields } from "/web/sources-arrs.js";

// ---------- Following ----------

// Artists followed for new albums, and when each was last looked up, failures first.
export async function musicFollowing(ctx) {
  const follows = await api("/wanted/artists/follows");
  if (!ctx.current()) return [];
  const failing = follows.filter((f) => f.error);
  const followRows = follows.map((f) =>
    h(
      "li",
      {},
      h(
        "div",
        {},
        h("strong", {}, f.name),
        " ",
        h("span", { class: "badge" }, f.monitor_new === "all" ? "Every new album" : "Albums from now on"),
        f.error
          ? h("p", { class: "error" }, `Last look failed ${when(f.checked_at)}: ${f.error}. It's tried again the next day.`)
          : h("p", { class: "note" }, f.checked_at ? `Last looked ${when(f.checked_at)}${f.since ? `, following since ${f.since}` : ""}.` : "Not looked for yet; the next look is within six hours."),
      ),
    ),
  );

  return [
    group(
      "Following",
      h(
        "p",
        { class: "note" },
        "Artists followed for new albums are looked up on MusicBrainz every day while matching is on; a new album joins the wanted list on its own.",
      ),
      follows.length
        ? [failing.length ? h("p", { class: "error" }, `${plural(failing.length, "look", "looks")} failed.`) : null, h("ul", { class: "rows" }, followRows)]
        : h("p", { class: "status" }, "No artists are followed. An artist is followed when they're monitored and set to want new albums."),
    ),
  ];
}

// ---------- Matching ----------

export async function musicMatching(ctx) {
  const albums = await api("/music/unmatched");
  if (!ctx.current()) return [];
  const rows = albums.map((a) => {
    const actions = h("div", { class: "row-actions" });
    const result = h("p", { class: "note", role: "status" });
    const match = (releaseId, label) =>
      confirmInline(actions, {
        question: `Match “${a.title}” to ${label}? Its tracks take that release's ids, and matching won't revisit it.`,
        confirm: "Match it",
        run: async () => {
          await api(`/music/albums/${a.id}/match`, { method: "PUT", body: { release_id: releaseId } });
          ctx.reload(`Matched “${a.title}”.`);
        },
      });
    const choices = a.candidates.map((c) => {
      const label = `${c.title} by ${c.artist}${c.year ? ` (${c.year})` : ""}, ${plural(c.tracks, "track", "tracks")}`;
      return h("li", {}, h("button", { type: "button", class: "quiet", onclick: () => match(c.release_id, label) }, `Match to ${label}`));
    });
    const byId = field(`match-${a.id}`, "Or a MusicBrainz release id", h("input", { autocomplete: "off", spellcheck: "false", placeholder: "a release's id from musicbrainz.org" }));
    const idForm = form([byId], "Match to this release", async () => {
      const id = byId.input.value.trim();
      if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(id)) {
        byId.fail("Paste a release id: 36 letters, digits and dashes, from the release's page on musicbrainz.org.");
        return null;
      }
      match(id, "that release");
      return null;
    });
    const none = h("button", { type: "button", class: "quiet" }, "It has no release");
    none.addEventListener("click", () =>
      confirmInline(actions, {
        question: `Record that “${a.title}” has no MusicBrainz release? It keeps its own tags, and matching stops trying.`,
        confirm: "Record no release",
        run: async () => {
          await api(`/music/albums/${a.id}/match`, { method: "DELETE" });
          ctx.reload(`“${a.title}” keeps its own tags.`);
        },
      }),
    );
    actions.append(none);
    return h(
      "li",
      {},
      h(
        "div",
        {},
        h("strong", {}, a.title),
        " by ",
        a.artist,
        " ",
        h("span", { class: "badge" }, a.status === "ambiguous" ? "More than one release fits" : "No release found"),
        h("p", { class: "note path" }, `${a.library}: ${a.folder}, ${plural(a.tracks, "track", "tracks")}.`),
        choices.length ? h("ul", { class: "plain choices" }, choices) : null,
        idForm,
        result,
      ),
      actions,
    );
  });
  return [
    group(
      "Albums to match",
      h("p", { class: "note" }, "Albums MusicBrainz matching couldn't settle on its own. Choose the release they are, paste a release's id, or record that there's none."),
      rows.length ? h("ul", { class: "rows" }, rows) : h("p", { class: "status" }, "Every album is matched. Albums show up here when matching can't tell which release they are."),
    ),
  ];
}

// ---------- Quality profiles and imports ----------

function qualities(p) {
  const allowed = p.qualities.filter((q) => q.allowed).map((q) => q.quality);
  return allowed.length ? `Takes ${allowed.join(", ")}.` : "Takes nothing yet.";
}

// A profile's ranks, lowest first: a group or a lone quality each, allowed or not together.
function rankItems(p) {
  const items = new Map();
  for (const q of p.qualities) {
    const item = items.get(q.rank) ?? { name: q.group ?? q.quality, members: [], allowed: q.allowed };
    item.members.push(q.quality);
    items.set(q.rank, item);
  }
  return [...items.entries()].sort(([a], [b]) => a - b).map(([, item]) => item);
}

// A profile's row swapped for a form that changes it; Cancel puts the row back.
function editing(ctx, p, row) {
  const before = [...row.childNodes];
  const back = () => {
    row.replaceChildren(...before);
    row.querySelector("button")?.focus();
  };
  const fromLidarr = p.source === "lidarr";
  const name = field(
    `music-profile-${p.id}-name`,
    "Name",
    h("input", { value: p.name, autocomplete: "off", maxlength: 100, readonly: fromLidarr }),
    fromLidarr ? "Named as in Lidarr: artists from Lidarr find their profile by this name." : null,
  );
  const upgrades = h("input", { type: "checkbox", id: `music-profile-${p.id}-upgrades`, checked: p.upgrade_allowed });
  const upgradesBox = {
    box: h("div", { class: "check" }, upgrades, h("label", { for: upgrades.id }, "Upgrade to better releases until the cutoff")),
    clear() {},
  };
  const items = rankItems(p);
  const boxes = items.map((item, i) => {
    const id = `music-profile-${p.id}-allow-${i}`;
    const box = h("input", { type: "checkbox", id, checked: item.allowed });
    const label = item.members.length > 1 || item.members[0] !== item.name ? `${item.name} (${item.members.join(", ")})` : item.name;
    return { item, box, el: h("div", { class: "check" }, box, h("label", { for: id }, label)) };
  });
  const allowedError = h("p", { class: "field-error", id: `music-profile-${p.id}-allowed-error`, hidden: true });
  const allowed = {
    box: h(
      "fieldset",
      { class: "choices", "aria-describedby": allowedError.id },
      h("legend", {}, "Allowed qualities, lowest first"),
      boxes.map((b) => b.el),
      allowedError,
    ),
    fail(message) {
      allowedError.textContent = message;
      allowedError.hidden = false;
      boxes[0]?.box.setAttribute("aria-invalid", "true");
    },
    clear() {
      allowedError.hidden = true;
      boxes[0]?.box.removeAttribute("aria-invalid");
    },
  };
  const cutoff = field(
    `music-profile-${p.id}-cutoff`,
    "Cutoff",
    h("select", {}, items.map((item) => h("option", { value: item.name, selected: item.name === p.cutoff }, item.name))),
    "Upgrades stop once an album is at least this good. It has to be allowed.",
  );
  const save = form([name, upgradesBox, allowed, cutoff], "Save changes", async () => {
    const chosen = boxes.filter((b) => b.box.checked);
    if (!name.input.value.trim()) {
      name.fail("Give the profile a name.");
      return null;
    }
    if (!chosen.length) {
      allowed.fail("Allow at least one quality.");
      return null;
    }
    if (!chosen.some((b) => b.item.name === cutoff.input.value)) {
      cutoff.fail("Choose a cutoff it allows, or allow this one.");
      return null;
    }
    try {
      await api(`/music/profiles/${p.id}`, {
        method: "PUT",
        body: {
          name: name.input.value.trim(),
          upgrade_allowed: upgrades.checked,
          cutoff: cutoff.input.value,
          allowed: chosen.flatMap((b) => b.item.members),
        },
      });
    } catch (err) {
      if (err.status === 409) {
        name.fail("Another profile has that name.");
        return null;
      }
      throw err;
    }
    ctx.reload(`Saved ${name.input.value.trim()}.${fromLidarr ? " Importing Lidarr's profiles leaves it as you set it." : ""}`);
    return null;
  });
  save.querySelector(".form-actions").append(h("button", { type: "button", class: "quiet", onclick: back }, "Cancel"));
  save.addEventListener("keydown", (event) => {
    if (event.key === "Escape") back();
  });
  row.replaceChildren(h("div", {}, h("h3", { class: "edit-title" }, `Change ${p.name}`), save));
  name.input.focus();
}

export async function musicProfiles(ctx) {
  const all = await api("/music/profiles");
  if (!ctx.current()) return [];
  const rows = all.map((p) => {
    const row = h("li");
    const actions = h("div", { class: "row-actions" });
    actions.append(
      h("button", { type: "button", class: "quiet", onclick: () => editing(ctx, p, row) }, "Edit"),
      h(
        "button",
        {
          type: "button",
          class: "quiet",
          onclick: () =>
            confirmInline(actions, {
              question: `Delete ${p.name}? Searches already made keep their results. It can't be deleted while an artist, a song or the request settings use it.`,
              confirm: `Delete ${p.name}`,
              keep: "Keep it",
              run: async () => {
                await api(`/music/profiles/${p.id}`, { method: "DELETE" });
                ctx.reload(`Deleted ${p.name}.`);
              },
            }),
        },
        "Delete",
      ),
    );
    row.append(
      h(
        "div",
        {},
        h("strong", {}, p.name),
        " ",
        h("span", { class: "badge" }, p.source === "lidarr" ? "From Lidarr" : "Made here"),
        h("p", {}, `Up to ${p.cutoff}${p.upgrade_allowed ? ", upgraded until then" : ", no upgrades"}.`),
        h("p", { class: "note" }, qualities(p)),
      ),
      actions,
    );
    return row;
  });
  const lossless = h("button", { type: "button", class: "quiet" }, all.some((p) => p.source === "preset") ? "Put “FLAC preferred” back as it ships" : "Make “FLAC preferred”");
  const losslessResult = h("p", { class: "form-result", role: "status" });
  lossless.addEventListener("click", async () => {
    lossless.disabled = true;
    try {
      await api("/music/profiles/presets/lossless", { method: "POST" });
      announce("Made FLAC preferred");
      ctx.reload("“FLAC preferred” is ready.");
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      losslessResult.textContent = err.message;
      losslessResult.classList.add("error");
      lossless.disabled = false;
    }
  });

  const profileFields = arrFields("lidarr-profiles", "Lidarr", 8686);
  const profileImport = form(profileFields, "Import Lidarr's profiles", async () => {
    const body = arrBody(profileFields);
    if (!body) return null;
    const r = await api("/import/lidarr/profiles", { method: "POST", body });
    ctx.reload(`Imported ${plural(r.profiles, "profile", "profiles")} from Lidarr.`);
    return null;
  });
  return [
    group(
      "Music profiles",
      h("p", { class: "note" }, "Ranked lowest first, as in Lidarr. A wanted artist uses the one chosen for them on their page."),
      rows.length ? h("ul", { class: "rows" }, rows) : h("p", { class: "status" }, "No music profiles yet. Make “FLAC preferred” or import Lidarr's."),
      h("div", { class: "form-actions" }, lossless, losslessResult),
    ),
    group("Import Lidarr's profiles", h("p", { class: "note" }, "Makes Melete's copies of Lidarr's profiles match it, except ones you've changed here, which stay as you set them."), profileImport),
  ];
}

// Importing Lidarr's artists and albums onto the wanted list (Music's Add new shows it).
export function lidarrArtists() {
  const listFields = arrFields("lidarr-list", "Lidarr", 8686);
  const listImport = form(listFields, "Import Lidarr's artists", async () => {
    const body = arrBody(listFields);
    if (!body) return null;
    const r = await api("/import/lidarr", { method: "POST", body });
    listFields[1].input.value = "";
    return [
      `Imported ${plural(r.artists, "artist", "artists")} (${r.monitored_artists} monitored) and ${plural(r.albums, "album", "albums")} (${r.monitored_albums} monitored, ${r.on_disk} already in a library).`,
      r.without_library ? `${r.without_library} had a root folder no library matched.` : "",
      r.removed_artists || r.removed_albums ? `Removed ${plural(r.removed_artists, "artist", "artists")} and ${plural(r.removed_albums, "album", "albums")} Lidarr no longer lists.` : "",
    ]
      .filter(Boolean)
      .join(" ");
  });
  return group(
    "Import Lidarr's artists",
    h("p", { class: "note" }, "Its artists and albums join the wanted list. Running it again updates them and removes ones Lidarr no longer lists; ones added another way are left alone."),
    listImport,
  );
}
