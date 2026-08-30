# Design discussion — consumer path, prover, persistence

2026-08-30. Working record of the session that produced [../roadmap.md](../roadmap.md).
The roadmap holds what we decided to build; this holds why, what we ruled out, and
what is still open.

## 1. Pool access — settled by doing it

No KYC, no form, no queue. Access to the pool is one on-chain registration of the
viewing key, and it happens automatically on first use inside a privacy-enabled
wallet (Ready, formerly Argent; Xverse). Verified live on 2026-08-30:

- `get_fee_amount` -> `0x53444835ec580000` = **6 STRK per pool transaction**
- `get_screener_public_key` -> `0x501cc452...f88fdb2` (non-zero: screening enforced)
- `get_version` -> `2.0`

Our own registration + shield: tx `0x40093f3e154d77beab5cd7cfc8fe5c907efb2e28e8c5908bd04ea2d1485d19d`,
block 14093171, `SUCCEEDED`. Registration (`ViewingKeySet`), deposit (`Deposit`, 10 STRK)
and note creation (`EncNoteCreated`) all landed in **one** transaction — so it yields one
hash, not three. Of the 10 STRK deposited, 6 left immediately as the pool fee, leaving
~4 STRK shielded; gas was 3.13 STRK.

## 2. Screening — only deposits

`_apply_actions` returns a screening subject for `TransferFrom` (a regular deposit) and
for open-note deposits. For everything else the contract asserts `screening.is_none()`
(`UNEXPECTED_SCREENING`) — the attestation is not merely unnecessary there, it is
rejected. The attestation is a SNIP-12 `DepositorValidation{depositor, issued_at}` signed
by FPI, max age 300 s, +60 s future skew. It is address screening (Elliptic AML), not
identity KYC: the failure mode is "your address is flagged", not "your paperwork is
pending".

## 3. Prover — what is actually true

- The **hosted mainnet prover URL is not published**. Upstream's own
  `demo/.env.mainnet.example` still reads `VITE_PROVING_SERVICE_URL=TODO_MAINNET_PROVER_URL`.
  StarkWare hands it to teams on request; the one team we found with it documents it as
  "StarkWare's endpoint, not in the repo, **not to be shared**". We asked in
  [#124](https://github.com/starkience/strk20-hackathon/issues/124) rather than trying to
  lift it out of a wallet bundle — an endpoint obtained that way is revoked exactly when
  the demo needs it.
- Wallets need no URL from us because the Wallet API route reaches the wallet's own
  prover. AVNU is on that same route.
- **The prover is self-hostable**: `ghcr.io/starkware-libs/starknet-privacy/transaction-prover`,
  `docker run -e RPC_URL=<node v0.10>`, no credentials. StarkWare recommends
  c4d-highcpu-48 for production; ~29 s per proof on 12 cores / 46 GiB; a hackathon team on
  2 vCPU measured 5-7 minutes.
- **The hosted prover mints the screening attestation itself** (a `proof-interceptor`
  sidecar) and packs it automatically. A self-hoster has no partner credentials, so a
  self-hosted prover can register, transfer, withdraw and invoke — but **cannot shield**.
- **The prover receives the pool private key** in `compile_actions` calldata, and so does
  the preflight RPC. A hosted prover is therefore a permanent confidentiality dependency,
  with no rotation and no revocation.

Consequence for us: a write path built on the hosted prover would hand away the exact
property this project sells. A write path built on a self-hosted prover keeps it but
cannot shield.

## 4. Decision: no write path in our binary

Cut deliberately. Signing, key custody and prover operation are the surface this project
exists to avoid, and adding them would complicate the indexer with work that is not its
nature.

Instead we are **the read half of every write**. The SDK cannot build a spend without
knowing your notes, and that is what we supply keylessly. Shield stays a one-click wallet
action by the user. After submission, our subscription is what reports that the nullifier
landed and no reorg touched it. Write flows are incomplete without us while we touch
neither keys nor prover.

## 5. Architecture — two blocks, one seam

Earlier framing ("one state machine, two hosts") was wrong and was corrected in
discussion. The right decomposition:

- **Block A — ingest.** Node -> mirror -> feed. Backend only, never in a browser.
- **Block B — consumer state machine.** Fold feed -> local mirror -> run the unmodified
  `discovery-core` -> notes/balances/spent. Runs on the backend (delegated mode) and in
  the browser (keyless mode).

The seam already exists: the `FeedTransport` trait, with `HttpTransport` and
`DirTransport` today. The backend gains a third impl that reads its own DB in-process.
The browser gets none — TypeScript does the fetching.

**Browser split.** WASM is a pure synchronous computer: bytes in, notes out. No network,
no storage, no async JS inside Rust. Every `await` — IndexedDB, SSE, cache lookups —
lives in the TypeScript wrapper, because at call time the wrapper does not yet know
whether it must go to the network.

The spike confirmed the boundary from the other side: `crates/client` is not
wasm-portable (bundled SQLite via `rusqlite`, and `tokio::task::spawn_blocking` inside
`ClientView`'s `RawStorageAccess`). That is a confirmation of the design, not a problem
to fix — the browser needs an in-memory view, which is precisely the part being lifted
out.

## 6. Two APIs, deliberately

| | key | who computes | positioning |
|---|---|---|---|
| Keyless (default) | stays in the browser | client | the thing we sell |
| Delegated | goes to a server you run | server | self-host / SDK compat |

Naming: `KeylessClient` / `DelegatedClient`. "Delegated" names the mechanic — what you
handed over — where "local/remote" describes geography and "trusted/trustless" reads as a
verdict.

npm: zero packages published under the `strk20` scope, but scope ownership is not
checkable without a login (npm 403s org pages). Unscoped `strk20-discovery`,
`strk20-indexer` and `strk20` are all free. Leaning to unscoped `strk20-discovery`:
no dependency on someone else's org, and no implication of being official.

## 7. Persistence — discussed, not yet planned

The problem: an in-memory view is fine until the tab unloads, and re-downloading ~6 MB on
every load is not acceptable.

**What falls out of the epoch rule.** Epochs are cut only below `l1_accepted`, so they are
immutable by construction; only the tail (`head.ndjson`) can be rewritten. Therefore:

- persist only what is folded from epochs — it can never go stale;
- never persist the tail (<=10k blocks, ~16 KB at today's volume) — refetching it each
  load is cheaper than writing rollback code.

Consequence worth stating loudly: **the browser client needs no reorg logic at all.** No
rewind, no generation counter, no checkpoint cursors — a reorg cannot reach persisted
state. The browser client comes out simpler than the server one, not more complex.

**What IndexedDB holds** (all opaque blobs to TypeScript): the epoch-derived state; the
per-key discovery cursors and note registry (without them every load re-walks all
channels); and a compatibility stamp (format version, chain id, hash of the last applied
epoch) so a stale or foreign blob is rejected loudly.

Note on the registry: it is key-derived. On a shared machine it is a fingerprint. At
minimum document it; better, encrypt the blob under a key derived from the viewing key so
it is noise without the key.

**Open trade-off: raw epochs vs folded mirror.** The hash chain is over epoch payloads and
folding is irreversible, so a mirror-only cache means trusting our own IndexedDB blob,
which same-origin code can tamper with. Storing raw epochs re-verifies the chain on every
load. Proposal: raw epochs as the source of truth, folded mirror as a cache keyed by the
chain-head hash — fast path from cache, refold when it is missing or mismatched.

**The measurement that decides it:** how long folding the full history takes inside WASM.
At ~200 ms the mirror cache is unnecessary and a whole layer disappears; at ~2 s it is
mandatory. This should be measured before any TypeScript is written.

**Sketch of the WASM boundary** (module owns the format, TS stores bytes):

```
load(blob) -> ok | stale
apply_epoch(bytes) / apply_head(bytes)
export() -> blob              // called once per epoch (~4.7 h), not per poll
discover(owner, key, cursor_blob) -> notes, new_cursor_blob
```

`export()` cadence matters: writing megabytes to IndexedDB on every head poll would be
the obvious way to make this slow.

## 8. Deferred, with triggers

- **OHTTP** for delegated mode. Pointless in keyless mode — every client fetches identical
  bytes, there is nothing to hide. Trigger: delegated mode gains users who are not the
  operator.
- **Prefix-bucket endpoint, then PIR** (`../research/q9-pir.md`). Trigger: snapshot beyond
  ~50 MB, roughly 8x10^5 records.

## 9. Still open

1. Fold time for full history inside WASM (decides section 7).
2. Whether a self-hosted prover's output passes the live pool's proof-facts allowlist —
   the image tag is RC.2 while the deployed class is V2; a mock proof is rejected with
   "Proof version PROOF0 is not allowed under this protocol version".
3. Whether StarkWare confirms the shield/self-host split as intended (asked in #124).
4. Snapshot cadence and format, once snapshots exist: which block boundaries, and what
   exactly the anchor commits to.
