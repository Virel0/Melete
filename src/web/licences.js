// Licences: the libraries the web UI serves from src/web/vendor, each with its licence, where it
// came from and the full text that ships beside it. A test checks this list against the vendored
// files, so a library added there has to be named here.

import { group } from "/web/forms.js";
import { api, h } from "/web/lib.js";

// What each licence file holds, served as plain text from /web/licences/.
const TEXTS = {
  hls: { path: "/web/licences/hls.js.txt", name: "hls.js's licence" },
  jassub: { path: "/web/licences/jassub.txt", name: "JASSUB's licences" },
};

// Each vendored library: name, version, SPDX licence, what it does here, where it came from.
const LIBRARIES = [
  {
    name: "hls.js",
    version: "1.7.3",
    licence: "Apache-2.0",
    use: "Plays video streams in the web player.",
    source: "npm package hls.js 1.7.3 (dist/hls.light.min.mjs), unchanged",
    text: "hls",
  },
  {
    name: "jassub",
    version: "2.5.21",
    licence: "LGPL-2.1-or-later AND (FTL OR GPL-2.0-or-later) AND MIT AND MIT-Modern-Variant AND ISC AND NTP AND Zlib AND BSL-1.0",
    use: "Draws ASS and SSA subtitles in the web player: jassub's own code (MIT) and libass with the libraries it uses, compiled to WebAssembly, under the licences jassub's package states for them.",
    source: "npm package jassub 2.5.21, bundled with esbuild",
    text: "jassub",
  },
  {
    name: "abslink",
    version: "1.3.1",
    licence: "Apache-2.0",
    use: "Bundled into jassub.",
    source: "npm package abslink 1.3.1; https://github.com/ThaUnknown/abslink",
    text: "jassub",
  },
  {
    name: "rvfc-polyfill",
    version: "1.0.8",
    licence: "GPL-3.0",
    use: "Bundled into jassub.",
    source: "npm package rvfc-polyfill 1.0.8",
    text: "jassub",
  },
  {
    name: "throughput",
    version: "1.0.2",
    licence: "MIT",
    use: "Bundled into jassub.",
    source: "npm package throughput 1.0.2; https://github.com/ThaUnknown/throughput",
    text: "jassub",
  },
  {
    name: "Liberation Sans",
    version: null,
    licence: "OFL-1.1",
    use: "The font for subtitle text whose own font isn't available.",
    source: "npm package jassub 2.5.21 (default.woff2)",
    text: "jassub",
  },
];

export async function licences() {
  const { version } = await api("/health");
  return [
    group(
      "Melete",
      h("p", {}, `Version ${version}.`),
      h("p", { class: "note" }, "The web UI's own code is Melete's. These libraries are part of it too, served by Melete itself; nothing is fetched from anywhere else."),
    ),
    group(
      "Third-party libraries",
      h(
        "ul",
        { class: "rows" },
        LIBRARIES.map((lib) =>
          h(
            "li",
            {},
            h(
              "div",
              {},
              h("strong", {}, lib.name),
              " ",
              h("span", { class: "badge" }, lib.licence),
              h("p", {}, lib.use),
              h("p", { class: "note" }, `${lib.version ? `Version ${lib.version}. ` : ""}From the ${lib.source}. Full text: ${TEXTS[lib.text].name}, below.`),
            ),
          ),
        ),
      ),
    ),
    group("Licence texts", ...Object.values(TEXTS).map(fullText)),
  ];
}

// A licence file's text, fetched the first time it's opened.
function fullText({ path, name }) {
  const body = h("pre", { class: "report-details", tabindex: "0" }, "Loading…");
  const box = h("details", { class: "licence-text" }, h("summary", {}, name), body);
  let asked = false;
  box.addEventListener("toggle", async () => {
    if (!box.open || asked) return;
    asked = true;
    try {
      const response = await fetch(path, { credentials: "same-origin" });
      if (!response.ok) throw new Error();
      body.textContent = await response.text();
    } catch {
      asked = false;
      body.textContent = "Melete didn't send the text. Close this and open it again to retry.";
    }
  });
  return box;
}
