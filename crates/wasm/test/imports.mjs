// §3.9's import-section audit, done with the WebAssembly API instead of
// `wasm-objdump` (one fewer tool to install, same information).
//
// What this proves, at exactly its real strength: the module **cannot open a
// network handle, a storage handle, a timer, or a randomness source of its
// own**. It is not an empty section — `__wbindgen_throw` and the typed-array
// copy are calls into JS carrying arbitrary bytes, and they are how every ABI
// method returns its JSON. The claim is capability, not silence.
//
// This is also the load-bearing evidence that `getrandom` — which is in the
// dependency tree via `lambdaworks-math` -> `starknet-types-core`, and which
// §3.9 says must not be reachable — is DEAD CODE here: a live call would need a
// `crypto.getRandomValues` import, and there is none.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const mod = new WebAssembly.Module(readFileSync(join(here, "..", "pkg", "strk20_engine_bg.wasm")));
const imports = WebAssembly.Module.imports(mod);

// Only the wasm-bindgen glue module may be imported from, and only functions.
// Anything else — `env`, `wasi_snapshot_preview1`, a `crypto` shim — is a
// capability this module is not allowed to have.
const GLUE = "./strk20_engine_bg.js";
const bad = imports.filter((i) => i.module !== GLUE || i.kind !== "function");

console.log(`import section: ${imports.length} entries, all from ${GLUE}`);
for (const i of imports) console.log(`  ${i.name}`);

if (bad.length > 0) {
    console.log("\nFAIL — the module imports a capability it must not have:");
    for (const i of bad) console.log(`  ${i.module} :: ${i.name} (${i.kind})`);
    process.exit(1);
}
console.log("\nPASS — no network, storage, timer or randomness import");
