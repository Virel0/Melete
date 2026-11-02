// Play trailer on a title's page: a trailer in the title's folder, played in Melete's own
// player. A title without one has no button.
//
// trailerButton(kind, title, extras) → an element holding the button, or empty when there's no
// trailer; `kind` is "movie" or "series".

import { h } from "/web/lib.js";
import { playHref } from "/web/cards.js";

export function trailerButton(kind, title, extras) {
  const box = h("span", { class: "trailer-box" });
  const local = extras.find((e) => e.kind === "trailer");
  if (local) {
    box.append(h("a", { class: "button quiet", href: playHref(local.id, kind === "movie" ? { movie: title.id } : { series: title.id }) }, "Play trailer"));
  }
  return box;
}
