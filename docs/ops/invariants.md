# Invariant checks

Run from the repository root with Python 3.9+:

```sh
python3 scripts/check-invariants.py
```

The [script](../../scripts/check-invariants.py) reports `PASS`, `WARN` or `FAIL`
for each check and prints details for findings. Any `FAIL` produces exit status 1;
`WARN` is advisory and needs review. Passing means the implemented static checks
found no violation. It does not prove runtime correctness, anonymity or complete
secret detection. [CI](../../.github/workflows/ci.yml) also runs builds and tests.

The secret check reads configured local key locations. In a checkout where access
to keys is prohibited, run it in a clean CI environment without those locations,
or run the non-secret checks separately and clearly report that limitation.

## 1. Feed URL checks

The script parses `PATTERNS` from
[`feed_urls.rs`](../../crates/e2e-tests/src/feed_urls.rs), then checks selected
Rust string literals in client, consumer, WASM and feed sources. It rejects
recognized user-derived interpolation, query strings and paths outside that
allowlist, including literals passed to recognized fetch primitives.

This is a static check of selected Rust call shapes, not proof of every runtime
request or TypeScript path. Public URLs omit wallet selectors, but clients may
request different public ranges depending on cache and timing. Feed hosts still
observe network metadata. TypeScript transport and capture tests provide
additional coverage.

On failure, remove user-specific selectors from transport and filter locally.
Add an allowlisted public artifact only when its parameterless contract is
intentional and documented in [Architecture](../spec/architecture.md#http-interfaces).

## 2. WASM portability boundary

The script searches production code and direct dependency tables in `consumer`
and `wasm` for selected native dependencies and filesystem APIs. Comments, string
literals and test modules are excluded. This catches known violations; a real
WASM build is still necessary to establish target compatibility.

Move host I/O to native transport or the TypeScript host, passing bytes through
[`FeedTransport`](../../crates/consumer/src/transport.rs). Keep test-only
capabilities in dev dependencies rather than production dependencies.

## 3. Truncation and discarded-error checks

The scan requires the continuation-token guard to remain present. It also flags
`.take(...)` on recognized chain-data paths and discarded results from recognized
persistence calls. Other `.take(...)`, discarded results and apparently unguarded
`break` statements are warnings.

Presence and lexical matching do not prove all control paths are sound. Investigate
hard failures as possible data loss; read warnings in context and retain a short
explanation for intentional bounds or ignored errors. An event scan's success
cannot establish coverage of eventless writes; see
[ingestion](../spec/architecture.md#ingestion-and-reorgs).

## 4. Tracked secret checks

The script harvests secret-named values from matching files in gitignored local
locations (`data`, the user's `.strk20` directory, `.env`, and example directories),
then compares those values with tracked text. It separately searches for selected
secret-assignment shapes, private-key headers and provider tokens. Public-named
fields are excluded from harvesting, and some file classes are skipped.

Without local key files, only the shape check can detect a new secret. Neither
pass covers all encodings or secret types. On a finding, avoid printing the value;
confirm exposure, revoke or rotate affected credentials, remove the tracked
material and follow the appropriate repository-history cleanup process. Do not
add an exception merely to hide a real key.

## 5. Fork pin

The offline check requires the active fork redirect, a fixed 40-hex revision and
one commit in the patch file. It does not establish source equality or patch-file
freshness. The networked fork workflow checks source equality and the branch pin;
see [Fork maintenance](fork.md#check-the-patch) for its exact scope and remediation.

## 6. Compose RPC defaults

The four named Mainnet/Sepolia primary/fallback substitutions in
[Compose](../../docker-compose.yml) must match their constants in
[`config.rs`](../../crates/indexerd/src/config.rs). Missing variables or missing
defaults fail. The script does not evaluate `.env` overrides, endpoint capabilities,
WebSocket/feeder settings or which service uses a substitution.

On failure, reconcile the intended defaults in both files. Removing the default
only hides configuration drift; it does not make the deployment equivalent.
