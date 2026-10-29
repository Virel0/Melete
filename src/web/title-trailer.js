// Play trailer on a title's page. A trailer in the title's folder comes first and plays in
// Melete's own player. Without one, and only while an admin has turned trailers online on, the
// one TMDB lists plays in YouTube's privacy-enhanced player (youtube-nocookie.com): nothing is
// loaded from YouTube until Play trailer is pressed, and then only inside a frame in a dialog,
// which is the one frame the page's policy lets in while the setting is on. Closing the dialog
// takes the frame away, so the trailer stops.
//
// trailerButton(kind, title, extras) → an element that holds the button once it's known
// (empty when there's no trailer); `kind` is "movie" or "series".

import { SignedOut, api, h } from "/web/lib.js";
import { playHref } from "/web/cards.js";
import { icon } from "/web/music-controls.js";
import { signIn } from "/web/page-state.js";

const PLAYER = "https://www.youtube-nocookie.com";
let framesAllowed = null;

// Whether the page's policy lets YouTube's player in, which it does only while trailers online
// is on: asked of the page itself once, so with the setting off no title asks for a trailer.
function trailersOn() {
  framesAllowed ??= fetch("/", { method: "HEAD", credentials: "same-origin" })
    .then((r) => (r.headers.get("Content-Security-Policy") ?? "").includes(`frame-src ${PLAYER}`))
    .catch(() => false);
  return framesAllowed;
}

export function trailerButton(kind, title, extras) {
  const box = h("span", { class: "trailer-box" });
  const local = extras.find((e) => e.kind === "trailer");
  if (local) {
    box.append(h("a", { class: "button quiet", href: playHref(local.id, kind === "movie" ? { movie: title.id } : { series: title.id }) }, "Play trailer"));
    return box;
  }
  trailersOn()
    .then((on) => (on ? api(`/${kind === "movie" ? "movies" : "series"}/${title.id}/trailer`) : null))
    .then((trailer) => {
      // Only YouTube's privacy-enhanced player, whatever the answer says.
      if (trailer?.site !== "youtube" || !trailer.embed_url.startsWith(`${PLAYER}/embed/`)) return;
      box.append(...onlineButton(title, trailer));
    })
    .catch((err) => {
      // 404: off, or no trailer for this title. Either way there's no button.
      if (err instanceof SignedOut) signIn("Your session ended. Sign in again.");
    });
  return box;
}

// Play trailer, opening a dialog that loads YouTube's player only now.
function onlineButton(title, trailer) {
  const button = h("button", { type: "button", class: "quiet" }, icon("play"), "Play trailer");
  const frameBox = h("div", { class: "trailer-frame" });
  const close = h("button", { type: "button", class: "icon", "aria-label": "Close the trailer", title: "Close" }, icon("close"));
  const dialog = h(
    "dialog",
    { class: "trailer-dialog", "aria-labelledby": "trailer-head" },
    h("div", { class: "trailer-head" }, h("h2", { id: "trailer-head" }, `Trailer: ${title.title}`), close),
    frameBox,
    h("p", { class: "note" }, "Playing from YouTube, which sees this device and this trailer, and nothing about the library."),
  );
  const shut = () => {
    // The frame goes with the dialog, so the trailer stops and YouTube hears no more.
    frameBox.replaceChildren();
    if (dialog.open) dialog.close();
  };
  close.addEventListener("click", shut);
  dialog.addEventListener("close", () => {
    frameBox.replaceChildren();
    button.focus();
  });
  button.addEventListener("click", () => {
    frameBox.replaceChildren(
      h("iframe", {
        src: `${trailer.embed_url}?autoplay=1&rel=0`,
        title: trailer.name || `Trailer: ${title.title}`,
        allow: "autoplay; encrypted-media; picture-in-picture; fullscreen",
        allowfullscreen: true,
      }),
    );
    dialog.showModal();
    close.focus();
  });
  return [button, dialog];
}
