// Wanted songs, for admins: added here, searched for on Soulseek, and given a YouTube video by
// hand when Melete couldn't find one there itself.

import { confirmInline, field, form, group, when } from "/web/forms.js";
import { SignedOut, announce, api, h } from "/web/lib.js";
import { runSearch } from "/web/wanted-music.js";

// "4:23" or "263" to seconds; null when empty; NaN when it's neither.
function seconds(text) {
  const t = text.trim();
  if (!t) return null;
  const m = /^(\d{1,3}):([0-5]\d)$/.exec(t);
  if (m) return Number(m[1]) * 60 + Number(m[2]);
  return /^\d{1,5}$/.test(t) ? Number(t) : Number.NaN;
}

function songRow(ctx, song, libraries) {
  const results = h("div");
  const search = h("button", { type: "button", class: "quiet" }, "Search Soulseek");
  search.addEventListener("click", () =>
    runSearch(ctx, search, results, `/wanted/songs/${song.id}/search`, "Asking Soulseek; this can take up to a minute…", song.title),
  );
  const actions = h("div", { class: "row-actions" }, search, youtubeButton(song, results));
  const remove = h("button", { type: "button", class: "quiet" }, "Remove");
  remove.addEventListener("click", () =>
    confirmInline(actions, {
      question: `Stop wanting “${song.title}”? Its searches go with it.`,
      confirm: "Remove song",
      run: async () => {
        await api(`/wanted/songs/${song.id}`, { method: "DELETE" });
        ctx.reload(`Removed “${song.title}”.`);
      },
    }),
  );
  actions.append(remove);
  const library = libraries.find((l) => l.id === song.library_id);
  const facts = [
    song.album ? `From ${song.album}` : "On its own",
    song.length_seconds ? `${Math.floor(song.length_seconds / 60)}:${String(song.length_seconds % 60).padStart(2, "0")} long` : null,
    library ? `goes to ${library.name}` : "no library",
    `added ${when(song.added_at)}`,
  ].filter(Boolean);
  return h(
    "li",
    {},
    h("div", {}, h("strong", {}, song.title), " by ", song.artist, h("p", { class: "note" }, `${facts.join(", ")}.`), results),
    actions,
  );
}

// "Use a YouTube video": for a song Melete couldn't match on YouTube (or matched wrongly), a
// link an admin found, fetched as the song without matching it. Opens a small form in place.
function youtubeButton(song, results) {
  const open = h("button", { type: "button", class: "quiet" }, "Use a YouTube video");
  open.addEventListener("click", () => {
    const id = `song-youtube-${song.id}`;
    const input = h("input", { id, type: "url", autocomplete: "off", placeholder: "https://www.youtube.com/watch?v=…" });
    const said = h("p", { class: "note", role: "status" });
    const fetch = h("button", { type: "submit" }, "Fetch it as this song");
    const cancel = h("button", { type: "button", class: "quiet" }, "Cancel");
    const box = h(
      "form",
      { class: "youtube-pick" },
      h("label", { for: id }, `YouTube video for “${song.title}”`),
      h("p", { class: "note" }, "Fetched as it is, without checking it's the song or its length: only the video's id goes to YouTube. Needs songs from YouTube on."),
      input,
      h("div", { class: "row-actions" }, fetch, cancel),
      said,
    );
    cancel.addEventListener("click", () => {
      box.remove();
      open.focus();
    });
    box.addEventListener("submit", async (event) => {
      event.preventDefault();
      if (!input.value.trim()) {
        said.textContent = "Paste the video's link.";
        input.focus();
        return;
      }
      fetch.disabled = true;
      try {
        await api(`/wanted/songs/${song.id}/youtube`, { method: "POST", body: { video: input.value.trim() } });
        said.textContent = "Chosen. It's fetched shortly, once nothing else is with the downloader.";
        announce(`The video for ${song.title} is chosen.`);
      } catch (err) {
        if (err instanceof SignedOut) throw err;
        said.textContent = err.message;
        input.focus();
      } finally {
        fetch.disabled = false;
      }
    });
    results.replaceChildren(box);
    input.focus();
  });
  return open;
}

export async function wantedSongs(ctx) {
  const [songs, profiles, libraries] = await Promise.all([api("/wanted/songs"), api("/music/profiles"), api("/libraries")]);
  if (!ctx.current()) return [];
  const music = libraries.filter((l) => l.kind === "music");

  const title = field("song-title", "Title", h("input", { autocomplete: "off" }));
  const artist = field("song-artist", "Artist", h("input", { autocomplete: "off" }));
  const album = field("song-album", "Album (optional)", h("input", { autocomplete: "off" }), "With it, the song goes with that album; without it, in the artist's Singles folder.");
  const length = field(
    "song-length",
    "Length (optional)",
    h("input", { inputmode: "numeric", autocomplete: "off", class: "year" }),
    "Like 4:23. Tells a radio edit or a live take from it.",
  );
  const library =
    music.length > 1
      ? field(
          "song-library",
          "Library",
          h("select", {}, h("option", { value: "" }, "Where the artist's albums go"), music.map((l) => h("option", { value: l.id }, l.name))),
          "Needed unless the artist is a wanted artist.",
        )
      : null;
  const profile = field(
    "song-profile",
    "Quality profile",
    h("select", {}, h("option", { value: "" }, "The artist's, else FLAC preferred"), profiles.map((p) => h("option", { value: p.id }, p.name))),
  );
  const fields = [title, artist, album, length, library, profile].filter(Boolean);
  const add = form(fields, "Add song", async () => {
    const got = { title: title.input.value.trim(), artist: artist.input.value.trim() };
    const secs = seconds(length.input.value);
    if (!got.title) title.fail("Enter the song's title.");
    if (!got.artist) artist.fail("Enter who it's by.");
    if (Number.isNaN(secs) || secs === 0) length.fail("Enter the length as minutes and seconds, like 4:23, or leave it empty.");
    if (!got.title || !got.artist || Number.isNaN(secs) || secs === 0) return null;
    const body = {
      ...got,
      ...(album.input.value.trim() ? { album: album.input.value.trim() } : {}),
      ...(secs ? { length_seconds: secs } : {}),
      ...(library?.input.value ? { library_id: Number(library.input.value) } : {}),
      ...(profile.input.value ? { music_profile_id: Number(profile.input.value) } : {}),
    };
    const added = await api("/wanted/songs", { method: "POST", body });
    ctx.reload(`Added “${added.title}” by ${added.artist}.`);
    return null;
  });

  return [
    group(
      "Add a song",
      h("p", { class: "note" }, "Songs are searched for on Soulseek only: torrents come as albums."),
      music.length ? add : h("p", { class: "status" }, "There's no music library to put songs in yet."),
    ),
    group(
      "Wanted songs",
      songs.length ? null : h("p", { class: "status" }, "No songs wanted yet."),
      ...filteredList(songs, {
        id: "song-filter",
        label: "Find a song",
        matches: (s, q) => `${s.title} ${s.artist} ${s.album ?? ""}`.toLowerCase().includes(q),
        row: (s) => songRow(ctx, s, libraries),
        noun: ["song", "songs"],
      }),
    ),
  ];
}
