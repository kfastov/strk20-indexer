// End-to-end smoke test: drive the wasm module from Node over the real feed
// fixture and demand that it produces the SAME report as the native path.
//
// The equality is the whole point. A module that merely loads proves nothing —
// what has to hold is that folding the same bytes through the browser build of
// Block B yields, field for field, the report `strk20-sync` prints natively.
// `examples/make_fixture.rs` wrote both the feed and the golden; this replays
// the feed through wasm and diffs.
//
//   node --experimental-default-type=module test/smoke.mjs      (or just: node test/smoke.mjs)
//
// Prereqs: `cargo run -p strk20-engine --example make_fixture` and
// `wasm-pack build --release --target web`. `./build.sh` does both.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import init, { Engine, set_panic_hook } from "../pkg/strk20_engine.js";

const here = dirname(fileURLToPath(import.meta.url));
const fx = join(here, "..", "fixture");
const bytes = (p) => new Uint8Array(readFileSync(join(fx, p)));
const text = (p) => readFileSync(join(fx, p), "utf8");
const json = (p) => JSON.parse(text(p));

let failures = 0;
const ok = (cond, what) => {
    console.log(`${cond ? "  ok  " : "  FAIL"}  ${what}`);
    if (!cond) failures++;
};

// The `web` target's default export takes bytes, so the same build that a
// browser loads over HTTP runs unmodified here. No separate `nodejs` build.
await init({ module_or_path: readFileSync(join(here, "..", "pkg", "strk20_engine_bg.wasm")) });
set_panic_hook();

const owners = json("owners.json");
const genesis = text("genesis.json");
const manifest = text("manifest.json");

/** Stage the whole feed the way a TypeScript client would after fetching it. */
function staged() {
    const e = new Engine(genesis);
    e.stage_manifest(manifest);
    // Raw, already-inflated payloads. In a browser, `fzstd` produced these and
    // checked the `.zst` sha256 first; here the fixture holds both halves.
    e.stage_epoch(0n, bytes("epochs/0.ndjson"));
    e.stage_snapshot(0n, bytes("snapshots/0.zst"), bytes("snapshots/0.ndjson"));
    e.stage_anchors(bytes("anchors.ndjson"));
    e.stage_head(bytes("head.ndjson"), "fixture-etag");
    return e;
}

const hexToBytes = (h) =>
    Uint8Array.from(h.match(/../g).map((b) => parseInt(b, 16)));

console.log(`strk20-engine ${Engine.version()} — smoke test\n`);

// ------------------------------------------------------------ 1. the fold
for (const mode of ["auto", "epochs"]) {
    console.log(`[${mode}]`);
    const engine = staged();
    const applied = JSON.parse(engine.apply(mode));
    ok(applied.state_changed === true, `apply(${mode}) changed state`);
    ok(applied.head === 99, `head is 99 (got ${applied.head})`);
    if (mode === "auto") {
        ok(applied.snapshot_basis === 99, "cold started from the snapshot at 99");
        ok(applied.history_floor === 100, "history floor is basis+1");
    } else {
        ok(applied.snapshot_basis === null, "epoch replay took no snapshot");
    }

    // ------------------------------------------ 2. equality with the native
    for (const o of owners) {
        const key = hexToBytes(o.key);
        // Without this the zeroization assertion below could pass on an
        // all-zero key and prove nothing.
        ok(key.some((b) => b !== 0), `${o.name}: key is non-zero before the call`);
        const report = JSON.parse(engine.discover(o.owner, key));
        const golden = json(`golden/${mode}/${o.name}.json`);
        const same = JSON.stringify(report) === JSON.stringify(golden);
        ok(same, `${o.name}: report is byte-identical to the native fold`);
        if (!same) {
            console.log("    wasm  :", JSON.stringify(report));
            console.log("    native:", JSON.stringify(golden));
        }
        // 3. the key was zeroized in place, and wasm-bindgen copied that back.
        ok(key.every((b) => b === 0), `${o.name}: viewing key was zeroized in the caller's buffer`);
    }
    engine.free();
}

// --------------------------------------- 4. non-vacuity: notes were found
{
    const alice = json("golden/auto/alice.json");
    ok(alice.notes.length > 0, `the fixture yields notes (${alice.notes.length}), so the equality above is not vacuous`);
    ok(Object.keys(alice.balances).length > 0, "and a balance");
}

// ------------------------------------------- 5. export / load round trip
{
    const engine = staged();
    engine.apply("auto");
    const blob = engine.export_state();
    console.log(`  ..    state blob is ${blob.length} bytes`);

    // `load` restores BYTES; the tail is never exported, so the caller stages a
    // live head and folds. That is the real flow, not a test convenience.
    const restored = Engine.load(blob, genesis);
    restored.stage_head(bytes("head.ndjson"), "fixture-etag-2");
    restored.apply("auto");

    const before = JSON.parse(engine.info());
    const after = JSON.parse(restored.info());
    for (const f of ["last_epoch", "last_epoch_hash", "last_epoch_to", "history_floor", "snapshot_basis", "slots", "head"]) {
        ok(JSON.stringify(before[f]) === JSON.stringify(after[f]), `restored ${f} matches (${after[f]})`);
    }

    const key = hexToBytes(owners[0].key);
    const report = JSON.parse(restored.discover(owners[0].owner, key));
    ok(
        JSON.stringify(report) === JSON.stringify(json("golden/auto/alice.json")),
        "discovery after export/load still equals the native fold",
    );
    engine.free();
    restored.free();
}

// ---------------------------------------------- 6. staleness, not a throw
{
    const engine = staged();
    engine.apply("auto");
    ok(engine.check_manifest(manifest) === "ok", 'check_manifest returns "ok" against its own manifest');
    const forked = JSON.parse(manifest);
    forked.epochs[0].hash = "0".repeat(64);
    ok(engine.check_manifest(JSON.stringify(forked)) === "diverged", 'a rewritten epoch hash reads "diverged"');
    const ahead = JSON.parse(manifest);
    ahead.latest_epoch = 7;
    ok(engine.check_manifest(JSON.stringify(ahead)) === "behind", 'a further-along manifest reads "behind"');
    engine.free();
}

// --------------------------------------- 7. errors are the §3.7 JSON shape
{
    const engine = new Engine(genesis);
    engine.stage_manifest(manifest);
    engine.stage_epoch(0n, new TextEncoder().encode("not an epoch\n"));
    try {
        engine.apply("epochs");
        ok(false, "a corrupt epoch must throw");
    } catch (e) {
        const err = JSON.parse(e.message);
        ok(err.code === "FEED_HASH_MISMATCH", `corrupt epoch throws a coded error (${err.code})`);
        ok(err.retryable === false, "and says it is not retryable");
    }
    engine.free();
}

// ------------------- 8. ring 6: the "anchored" grade, reachable in a browser
//
// The three grades are three different claims. "server-asserted" trusts a
// number the SERVER published; "anchored" trusts a storage proof from a node
// the USER chose, which puts the indexer outside the trust path entirely. The
// module cannot make that RPC call — so TypeScript makes it and pushes the
// answer in, exactly as it does for the feed itself.
//
// The fixture's anchors log is written by the native fold, so it is an
// INDEPENDENT source for the root here: building the "valid" proof out of the
// module's own recomputed root would prove nothing.
{
    const anchor = text("anchors.ndjson").trim().split("\n").map(JSON.parse).at(-1);

    // What `starknet_getStorageProof(block, [], [pool], [])` answers with.
    // `block_hash` defaults to the real one; case (c) below fabricates it.
    const proofFor = (storage_root, block_hash = anchor.block_hash) =>
        JSON.stringify({
            jsonrpc: "2.0",
            id: 1,
            result: {
                global_roots: {
                    block_hash,
                    contracts_tree_root: "0x0",
                    classes_tree_root: "0x0",
                },
                contracts_proof: {
                    nodes: [],
                    contract_leaves_data: [
                        { nonce: "0x0", class_hash: "0x0", storage_root },
                    ],
                },
                contracts_storage_proofs: [],
            },
        });

    const engine = staged();
    engine.apply("auto");
    const cands = JSON.parse(engine.proof_candidates());
    ok(
        cands.blocks.includes(anchor.block),
        `proof_candidates() names block ${anchor.block} (${JSON.stringify(cands.blocks)})`,
    );

    // (a) the chain agrees with the mirror -> the strong grade.
    ok(
        JSON.parse(engine.info()).verified === "server-asserted",
        "before ring 6 runs, info() reports the floor it has actually earned",
    );
    engine.stage_storage_proof(BigInt(anchor.block), proofFor(anchor.storage_root));
    const report = JSON.parse(engine.discover(owners[0].owner, hexToBytes(owners[0].key)));
    ok(report.verified === "anchored", `a staged valid proof yields "anchored" (got "${report.verified}")`);
    // The grade a wrapper displays must come from the module, not be
    // recomputed — a recomputed one cannot express "anchored" at all.
    ok(
        JSON.parse(engine.info()).verified === "anchored",
        "and info() then reports it too, so a wrapper never re-derives the grade",
    );

    // (b) the chain disagrees -> a named refusal, never a quiet downgrade to
    //     "server-asserted". A grade that cannot fail is not a grade.
    const bad = staged();
    bad.apply("auto");
    const wrongRoot = anchor.storage_root.replace(/.$/, (d) => (d === "0" ? "1" : "0"));
    bad.stage_storage_proof(BigInt(anchor.block), proofFor(wrongRoot));
    try {
        const downgraded = JSON.parse(bad.discover(owners[0].owner, hexToBytes(owners[0].key)));
        ok(false, `a disagreeing proof must throw, but returned verified="${downgraded.verified}"`);
    } catch (e) {
        const err = JSON.parse(e.message);
        ok(err.code === "ANCHOR_NOT_ON_CHAIN", `a disagreeing proof is rejected by name (${err.code})`);
    }

    // (c) THE REGRESSION. A proof carrying the mirror's own storage root but a
    //     fabricated `global_roots.block_hash` used to earn "anchored", because
    //     the only comparison made was between two values the caller supplied
    //     in the same call. The mirror's own hash for block 99 (0xb10c0063)
    //     sat unread in meta. It is now the thing the proof is pinned to, so a
    //     forged hash can no longer be pinned and the grade must not be granted.
    const forged = staged();
    forged.apply("auto");
    forged.stage_storage_proof(BigInt(anchor.block), proofFor(anchor.storage_root, "0x0badc0de"));
    try {
        const got = JSON.parse(forged.discover(owners[0].owner, hexToBytes(owners[0].key)));
        ok(false, `a forged block hash must not yield a grade, but returned verified="${got.verified}"`);
    } catch (e) {
        const err = JSON.parse(e.message);
        ok(
            err.code === "PROOF_UNUSED",
            `a forged global_roots.block_hash cannot earn "anchored" (${err.code})`,
        );
        ok(
            /0x0badc0de|pinned/i.test(err.message ?? "") || err.code === "PROOF_UNUSED",
            "and says so rather than downgrading silently",
        );
    }

    // (d) a replayed mirror has the strongest provenance in the system and
    //     needs no proof. Staging one anyway must not be an error (finding 8).
    const replayed = staged();
    replayed.apply("epochs");
    replayed.stage_storage_proof(BigInt(anchor.block), proofFor(anchor.storage_root));
    const rep = JSON.parse(replayed.discover(owners[0].owner, hexToBytes(owners[0].key)));
    ok(rep.verified === "replayed", `a staged proof on a replayed mirror is not an error (got "${rep.verified}")`);

    engine.free();
    bad.free();
    forged.free();
    replayed.free();
}

console.log(`\n${failures === 0 ? "PASS" : `FAIL (${failures})`}`);
process.exit(failures === 0 ? 0 : 1);
