// What a title's page keeps out of the way: links to its extras, cast and crew and collection
// (each on its own address), and About, folded, with the file details for the curious.
//
// - moreLinks(kind, title, extras, credits): "Extras (3)", "Cast and crew", "Part of …";
//   null when there's none of them.
// - aboutSection(title, files, { admin, kind }): About, folded; a file's details are asked
//   for when it's opened.
// - extrasPage(kind, id, gen) and castPage(kind, id, gen): `#/movies/5/extras`, `…/cast`.

import { SignedOut, announce, api, clock, fileName, h, languageName, loading, plural, runtime, show, size } from "/web/lib.js";
import { generation, signIn } from "/web/page-state.js";
import { playHref } from "/web/cards.js";
import { creditsSection } from "/web/people.js";

const EXTRA_KINDS = {
  trailer: "Trailer",
  featurette: "Featurette",
  behind_the_scenes: "Behind the scenes",
  deleted_scene: "Deleted scene",
  interview: "Interview",
  scene: "Scene",
  short: "Short",
  clip: "Clip",
  other: "Extra",
};

const route = (kind) => (kind === "movie" ? "movies" : "series");

export function moreLinks(kind, title, extras, credits) {
  const base = `#/${route(kind)}/${title.id}`;
  const links = [
    extras.length ? h("a", { class: "button quiet", href: `${base}/extras` }, `Extras (${extras.length})`) : null,
    credits.cast.length || credits.crew.length ? h("a", { class: "button quiet", href: `${base}/cast` }, "Cast and crew") : null,
    credits.collection ? h("a", { class: "button quiet", href: `#/collections/${credits.collection.id}` }, `Part of ${credits.collection.name}`) : null,
  ].filter(Boolean);
  return links.length ? h("nav", { class: "title-more", "aria-label": `More about ${title.title}` }, links) : null;
}

// "1080p H.264, English 5.1 AAC and Japanese AAC, 3 subtitle tracks": a file as its streams say.
function fileFacts(info) {
  const m = info.media;
  if (!m) return "Not read yet.";
  const v = m.video[0];
  const picture = v ? [v.height ? `${v.height}p` : null, v.codec?.toUpperCase(), v.hdr].filter(Boolean).join(" ") : null;
  const sound = m.audio
    .map((a) => [languageName(a.language), a.channel_layout, a.codec?.toUpperCase()].filter(Boolean).join(" "))
    .join(" and ");
  const words = [picture, sound || null, m.subtitles?.length ? plural(m.subtitles.length, "subtitle track", "subtitle tracks") : null];
  return words.filter(Boolean).join(", ");
}

// One file in About: its name and size, and what's in it once read.
function fileItem(file) {
  const facts = h("span", { class: "note" }, "Reading…");
  api(`/files/${file.id}`)
    .then((info) => (facts.textContent = [fileFacts(info), info.media?.duration_seconds ? clock(info.media.duration_seconds) : null].filter(Boolean).join(" · ")))
    .catch((err) => {
      if (err instanceof SignedOut) return signIn("Your session ended. Sign in again.");
      facts.textContent = "Its details couldn't be read.";
    });
  return h("li", {}, h("span", { class: "path", title: file.path }, fileName(file.path)), h("span", { class: "size" }, size(file.size)), facts);
}

// For admins: scans the title's folder alone now, for a file just copied in.
function rescanButton(kind, id) {
  const button = h("button", { type: "button", class: "quiet" }, "Rescan its folder");
  button.addEventListener("click", async () => {
    button.disabled = true;
    try {
      await api(`/${route(kind)}/${id}/rescan`, { method: "POST" });
      announce("Rescan asked for. Reload in a moment to see what it found.");
    } catch (err) {
      if (err instanceof SignedOut) return signIn("Your session ended. Sign in again.");
      announce(`Couldn't rescan. ${err.message}`);
    } finally {
      button.disabled = false;
    }
  });
  return button;
}

export function aboutSection(title, files, { admin = false, kind, unparsed = [] }) {
  const list = h("ul", { class: "files about-files" });
  const facts = [
    title.original_title && title.original_title !== title.title ? ["Original title", title.original_title] : null,
    kind === "movie" && title.runtime_minutes ? ["Length", runtime(title.runtime_minutes)] : null,
    title.status ? ["Status", title.status] : null,
    title.library ? ["Library", title.library] : null,
  ].filter(Boolean);
  const box = h(
    "details",
    { class: "about" },
    h("summary", {}, h("h2", {}, "About")),
    facts.length ? h("dl", { class: "about-facts" }, facts.map(([k, v]) => h("div", {}, h("dt", {}, k), h("dd", {}, v)))) : null,
    files.length ? [h("h3", {}, plural(files.length, "File", "Files")), list] : null,
    unparsed.length
      ? [
          h("h3", {}, "Files without an episode number"),
          h("p", { class: "note" }, "Melete couldn't tell which episode these are from their names."),
          h("ul", { class: "files" }, unparsed.map((f) => h("li", {}, h("span", { class: "path" }, fileName(f.path)), h("span", { class: "size" }, size(f.size))))),
        ]
      : null,
    admin ? h("p", {}, rescanButton(kind, title.id)) : null,
  );
  // Read only when someone opens it, a file at a time.
  box.addEventListener("toggle", () => {
    if (box.open && !list.childElementCount) list.replaceChildren(...files.slice(0, 20).map(fileItem));
  });
  return h("section", { class: "about-section", "aria-label": "About" }, box);
}

// A page of its own under the title: its heading, a way back, and what `body` draws.
async function underTitle(kind, id, gen, what, body) {
  show(what, loading(what.toLowerCase()));
  const [title, extra] = await Promise.all([api(`/${route(kind)}/${id}`), body.load()]);
  if (gen !== generation) return;
  show(
    `${what}: ${title.title}`,
    h("p", { class: "back" }, h("a", { href: `#/${route(kind)}/${id}` }, `Back to ${title.title}`)),
    h("h1", {}, `${what}: ${title.title}`),
    body.draw(extra, title),
  );
}

export function extrasPage(kind, id, gen) {
  return underTitle(kind, id, gen, "Extras", {
    load: () => api(`/${route(kind)}/${id}/extras`),
    draw: (extras) =>
      extras.length
        ? h(
            "ul",
            { class: "files extras" },
            extras.map((e) =>
              h(
                "li",
                {},
                h("a", { href: playHref(e.id, kind === "movie" ? { movie: id } : { series: id }) }, e.title),
                h("span", { class: "size" }, [EXTRA_KINDS[e.kind] ?? "Extra", e.duration_seconds ? clock(e.duration_seconds) : null].filter(Boolean).join(" · ")),
              ),
            ),
          )
        : h("p", { class: "status" }, "No extras for this title."),
  });
}

export function castPage(kind, id, gen) {
  return underTitle(kind, id, gen, "Cast and crew", {
    load: () => api(`/${route(kind)}/${id}/credits`),
    draw: (credits) => creditsSection(credits) ?? h("p", { class: "status" }, "No cast or crew known for this title."),
  });
}
