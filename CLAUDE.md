# strk20-indexer

Keyless note indexer for the STRK20 privacy pool on Starknet. The server mirrors pool storage
into a hash-chained public feed; the client folds that feed locally and runs the upstream
`discovery-core` engine, so the viewing key never leaves the wallet. Two binaries: `strk20`
(server, `crates/indexerd`), `strk20-sync` (client, `crates/client`).

## Build and test

```sh
cargo build --workspace --locked
cargo test --workspace
python3 scripts/check-invariants.py
cd ts && npm ci && npm run build
```

## Hard rules

- Never read `~/.strk20`, `data/**/vk*.txt`, `*keystore*`, `accounts.json`. Keys are not context.
- Never sign or submit a transaction. Not on mainnet, not on Sepolia, not to test.
- Never touch `data/mainnet` without the orchestrator's say-so; it takes hours to rebuild.
- Never run `git stash`, `checkout`, `reset` or `clean`. Other agents share this working tree.
- Do not commit unless asked. Leave changes in the working tree.

## How to work here

- Decisions go in the commit message, not a new markdown file.
- Research and council output goes to the session scratchpad, never into `docs/`.
- A document with no inbound link from README or a spec does not get created.
- Convene a council of proposals only for irreversible decisions.
- Never write a projection in the present tense as a measurement. Tag it or drop it.
- One claim gets one carrier, the cheapest that holds it: golden-byte unit test before e2e leg.
- Before cutting a test, find the commit where it failed. If it never failed, ask why.
- Every number comes from a file in the tree or a command you ran, or it is left out.

## verify-root has three outcomes, not two

- **MATCH**: recomputed storage root equals the proof; the server writes an anchor.
- **MISMATCH**: the mirror is missing writes; `verify_root_failed` latches, publishing stops.
- **UNAVAILABLE**: no endpoint served a proof; nothing latches, epochs are cut unverified.

Never write "refuses to publish if it disagrees" without the UNAVAILABLE case
(`crates/indexerd/src/cutter.rs`). The hole it catches: `getEvents` cannot name a block that
writes pool storage and emits no event (`docs/spec/sound-ingest.md`); repair is `rescan` then
`recut-epochs`.
