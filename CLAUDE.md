# strk20-indexer

Keyless note indexer for the STRK20 privacy pool on Starknet. The server mirrors pool storage
into a hash-chained public feed; the client folds that feed locally and runs the upstream
`discovery-core` engine, so the viewing key never leaves the wallet. Two binaries: `strk20`
(server, `crates/indexerd`), `strk20-sync` (client, `crates/client`).

## Build and test

```sh
cargo build --workspace --locked
cargo test --workspace --locked
```

For WASM, SDK and demo builds, follow [Build from source](README.md#build-from-source)
in order. For invariant checks, read [Invariants](docs/ops/invariants.md), including
the secret scanner's input scope; run the full scanner in a clean CI environment
when local key access is prohibited.

## Hard rules

- Never read `~/.strk20`, `data/**/vk*.txt`, `*keystore*`, `accounts.json`. Keys are not context.
- Never touch `data/mainnet` without the orchestrator's say-so; it takes hours to rebuild.
- Never run `git stash`, `checkout`, `reset` or `clean`. Other agents share this working tree.
- Commit and push completed, verified work autonomously. Preserve unrelated working-tree changes.

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

- **MATCH**: the reconstructed root agrees at the checked block; the server records an anchor.
- **MISMATCH**: `verify_root_failed` latches and the current epoch cut aborts; live-tail publication can continue.
- **UNAVAILABLE**: proof acquisition could not verify state; it does not change the latch and permits unverified epoch cuts.

For ingestion, publication or recovery work, read
[Architecture](docs/spec/architecture.md#verification-and-publication) and check
`crates/indexerd/src/cutter.rs`. A successful check covers state at one block, not
all historical writes. Operator repair instructions are in
[Hosting](docs/ops/hosting.md#repairing-a-mirror).

## Agent skills

These configuration files and domain documents created by `/domain-modeling` are
exceptions to the documentation rules above.

### Issue tracker

Issues and specs live in GitHub Issues for `kfastov/strk20-indexer`. Before issue
operations, read `docs/agents/issue-tracker.md`.

### Triage labels

Use the five standard triage labels. Before triaging issues, read
`docs/agents/triage-labels.md`.

### Domain docs

Use a single-context layout: root `CONTEXT.md` and `docs/adr/`. Before exploring
the codebase, read `docs/agents/domain.md`.
