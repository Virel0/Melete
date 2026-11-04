Decision: The web player draws ASS/SSA tracks with JASSUB 2.5.21, bundled into Melete
(`src/web/vendor/jassub-2.5.21`, with its build steps and licences) and served at a versioned
path. Fonts come only from the video and the bundled Liberation Sans: the Google Fonts lookup is
left out of the bundle and font queries are off. The page's policy is unchanged; the worker,
which alone compiles WebAssembly, is sent with its own policy adding `'wasm-unsafe-eval'`, and
JASSUB's page-side support check is changed to one that compiles nothing. If drawing can't start
within 15 seconds, or fails, the track's WebVTT version is shown.

Why: WebVTT keeps only the words of a styled track; libass draws it as authored. Serving
everything from Melete and confining WebAssembly to the worker keeps the rule that the web UI
loads nothing from outside and runs nothing evaluated on the page.

Date: 2026-11-04
