#!/usr/bin/env bash
# Build the npm package, regenerate the fixture, run the equality smoke test,
# and report the wire cost.
#
#   ./build.sh          build + test
#   ./build.sh --size   build + test + size report only
set -euo pipefail
cd "$(dirname "$0")"

echo "==> fixture + native golden"
cargo run -q -p strk20-engine --example make_fixture

echo
echo "==> wasm-pack (target web)"
wasm-pack build --release --target web --out-dir pkg --out-name strk20_engine

echo
echo "==> size (total published wire cost, §3.9's single denominator)"
total_gz=0
total_br=0
for f in pkg/strk20_engine_bg.wasm pkg/strk20_engine.js; do
    raw=$(wc -c < "$f")
    gz=$(gzip -9 -c "$f" | wc -c)
    br=$(brotli -q 11 -c "$f" | wc -c)
    total_gz=$((total_gz + gz))
    total_br=$((total_br + br))
    printf '  %-28s raw %8d   gzip %7d   brotli %7d\n' "$(basename "$f")" "$raw" "$gz" "$br"
done
printf '  %-28s              gzip %7d   brotli %7d\n' 'TOTAL (module + glue)' "$total_gz" "$total_br"
echo "  NOTE: a TypeScript wrapper must add a JS zstd decoder (fzstd, ~10 KB gzip)"
echo "        to reach the figure §3.9 gates. See README, \"Size\"."

echo
echo "==> import-section audit (§3.9)"
node test/imports.mjs

echo
echo "==> smoke test (wasm report must equal the native fold)"
node test/smoke.mjs
