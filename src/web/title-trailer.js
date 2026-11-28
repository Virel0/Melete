// Play trailer on a title's page: a trailer in the title's folder, played in Melete's own
// player. A title without one has no button.
//
// trailerButton(kind, title, extras) → an element holding the button, or empty when there's no
// trailer; `kind` is "movie" or "series".

import { h } from "/web/lib.js";
import { playHref } from "/web/cards.js";
import { icon } from "/web/music-controls.js";

export function trailerButton(kind, title, extras) {
  const box = h("span", { class: "trailer-box" });
  const local = extras.find((e) => e.kind === "trailer");
  if (local) {
    const href = playHref(local.id, kind === "movie" ? { movie: title.id } : { series: title.id });
    box.append(h("a", { class: "button tertiary with-icon", href }, icon("play"), "Play trailer"));
  }
  return box;
}
