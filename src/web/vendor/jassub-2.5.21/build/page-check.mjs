// After bundling: JASSUB's check that WebAssembly works compiles an empty module on the page,
// which Melete's page policy (no 'wasm-unsafe-eval') refuses. WebAssembly.validate answers the
// same question without compiling, so only the worker, under its own policy, compiles anything.
// Usage: node page-check.mjs ../jassub.mjs
import { readFileSync, writeFileSync } from "node:fs";
const file = process.argv[2];
const from = `    const module = new WebAssembly.Module(Uint8Array.of(0, 97, 115, 109, 1, 0, 0, 0));
    if (!(module instanceof WebAssembly.Module) || !(new WebAssembly.Instance(module) instanceof WebAssembly.Instance))
      throw new Error("WASM not supported");`;
const to = `    if (!WebAssembly.validate(Uint8Array.of(0, 97, 115, 109, 1, 0, 0, 0)))
      throw new Error("WASM not supported");`;
const code = readFileSync(file, "utf8");
if (code.split(from).length !== 2) throw new Error("the check isn't there exactly once: a new JASSUB?");
writeFileSync(file, code.replace(from, to));
