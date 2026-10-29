// The redesigned pages of Manage's Movies, Series, Anime and Music, by service and page key
// (manage-services.js lists them in each sidebar). Each draws itself whole, heading included,
// so admin.js only finds the page and puts it in its frame.

import { SignedOut, h, show } from "/web/lib.js";
import { failed, placeholders } from "/web/page-parts.js";
import { currentHealth } from "/web/health.js";
import { findingsOf } from "/web/manage-services.js";
import { libraryPage } from "/web/manage-library.js";
import { titlePage } from "/web/manage-title.js";
import { artistPage } from "/web/manage-artist.js";
import { albumPage } from "/web/manage-album.js";

import { calendarFor, cutoffFor, missingFor } from "/web/manage-activity.js";

const VIDEO = { library: libraryPage, title: titlePage, calendar: calendarFor, missing: missingFor, cutoff: cutoffFor };

export const MANAGE = {
  movies: VIDEO,
  series: VIDEO,
  anime: VIDEO,
  music: { library: libraryPage, artist: artistPage, album: albumPage, calendar: calendarFor, missing: missingFor },
};

// Draws `page` for `place` ({ service, item }) with what it needs: the address's query
// (`params`), the id after the page's key (`id`), the service's health findings, a way to
// name the tab after what's shown (`setTitle`), and `reload(message)` to draw it again saying
// what just happened.
export async function managePage(page, place, ctx) {
  const { service, item } = place;
  const body = h("div", { class: "manage-body" }, h("h1", {}, service.name), placeholders(item.name.toLowerCase(), { kind: "lines" }));
  show(`${item.name} – ${service.name}`, body);
  const fill = async (message) => {
    const found = await currentHealth().catch((err) => {
      if (err instanceof SignedOut) throw err;
      return null;
    });
    const parts = await page({
      ...ctx,
      service,
      findings: findingsOf(service, found),
      setTitle: (name) => (document.title = `${name} – ${service.name} – Melete`),
      reload: (said) => fill(said).catch(failing),
    });
    if (!ctx.current()) return;
    const flash = message ? h("p", { class: "flash", role: "status" }, message) : null;
    body.replaceChildren(...[flash, ...parts].flat(Infinity).filter(Boolean));
  };
  const failing = (err) => {
    if (err instanceof SignedOut) return ctx.signedOut();
    body.replaceChildren(h("h1", {}, service.name), failed(item.name.toLowerCase(), err, () => managePage(page, place, ctx)));
  };
  await fill().catch(failing);
}
