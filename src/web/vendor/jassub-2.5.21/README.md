# JASSUB 2.5.21, bundled

What the web player draws ASS/SSA subtitles with: libass compiled to WebAssembly, run in a
worker. Melete serves every file here itself; nothing is fetched from anywhere else.

Made from `jassub` 2.5.21 on npm with esbuild 0.25.10, both pinned with their integrity hashes
in `build/package-lock.json`:

```sh
cd build && npm ci --ignore-scripts
esbuild node_modules/jassub/dist/jassub.js --bundle --format=esm --platform=browser \
  --target=es2022 --alias:lfa-ponyfill=./no-remote-fonts.js --legal-comments=eof \
  --outfile=../jassub.mjs
esbuild node_modules/jassub/dist/worker/worker.js --bundle --format=esm --platform=browser \
  --target=es2022 --alias:lfa-ponyfill=./no-remote-fonts.js --legal-comments=eof \
  --outfile=../jassub-worker.mjs
node page-check.mjs ../jassub.mjs
cp node_modules/jassub/dist/wasm/jassub-worker.wasm \
   node_modules/jassub/dist/wasm/jassub-worker-modern.wasm \
   node_modules/jassub/dist/default.woff2 ..
```

`page-check.mjs` changes one thing in the page module: its check that WebAssembly works compiled
an empty module on the page, which the page's policy refuses; it now uses
`WebAssembly.validate`, which needs no compiling. Only the worker compiles WebAssembly, under
its own policy.

`lfa-ponyfill` asks Google Fonts for missing fonts; `build/no-remote-fonts.js` stands in for it,
so that code isn't in the bundle at all, and the player also sets `queryFonts: false`. Fonts
come from the video (`/stream/{token}/fonts`) and `default.woff2` (Liberation Sans).

A test checks each file's SHA-256 against what this made. Licences: `LICENSES.txt`.
