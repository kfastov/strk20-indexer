# Sepolia write path — feasibility of our own STRK20 pool transactions, headless

Research date: 2026-08-30. Question: can we produce **our own `EncNoteCreated` on the
Sepolia pool** (`0x0254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91`),
end to end, from this machine, with no browser wallet? **Research only — nothing below
was executed against the write path.**

Evidence discipline: **VERIFIED** = I observed it in this session (command + response
cited). **REPORTED** = a document or issue says it. **INFERRED** = follows from
verified/reported facts but was not directly observed.

## Verdict

**The chain is unbroken. Every link exists and is live.** A hosted, unauthenticated
Sepolia proving service is published (baked into the official demo's public bundle and
committed as a default in a hackathon team's repo), answers health checks today, and has
been used by a third party for the full register→shield→transfer lifecycle on this exact
pool. Funding is headless via the Starknet Foundation faucet's proof-of-work Agent API
(no captcha, no account). Account creation is headless via `sncast` (after a toolchain
upgrade — the installed 0.34.0 is RPC-0.7 era and demonstrably fails against today's
Sepolia). The SDK runs in plain Node with a raw private key. The concrete recipe is in
the final section, with risk flags.

The one **policy** caveat: the alpha-sepolia endpoints are StarkWare infrastructure that
answers unauthenticated but was never explicitly blessed — issue #121 asked "are these
intended for sprint teams?" and no maintainer answer exists in the thread as of today.
Technically nothing is broken; the risk is revocation/gating, not capability.

---

## Q1 — Sepolia proving service URL

### Upstream repo (starkware-libs/starknet-privacy)

- **Pinned checkout** (tag `CONTRACT_V2_DEPLOYED_MAINNET_2026-07-08`, rev `74841ca`,
  on disk at `~/.cargo/git/checkouts/starknet-privacy-c9b91989124c4d4d/74841ca`):
  `demo/.env.example` has `VITE_PROVING_SERVICE_URL=http://localhost:3000` and targets
  **SN_INTEGRATION_SEPOLIA** (`VITE_CHAIN_ID=0x534e5f494e544547524154494f4e5f5345504f4c4941`)
  — a StarkWare-internal network, *not* public Sepolia. `demo/.env.mainnet.example` still
  has `VITE_PROVING_SERVICE_URL=TODO_MAINNET_PROVER_URL`. **VERIFIED** (read local files).
  Do not copy demo `.env.example` values: wrong chain id for our target.
- **Current HEAD** (`f6cabbef`, committed 2026-08-30T08:41Z — checked via
  `api.github.com/repos/starkware-libs/starknet-privacy/commits`): the `demo/` directory
  still contains only `.env.example` and `.env.mainnet.example`; both fetched raw from
  `main` and unchanged in substance — **no Sepolia env file, mainnet prover still TODO**.
  **VERIFIED** (GitHub API dir listing + raw fetches).

### The published endpoints (via starkience/strk20-hackathon)

Issue **#121** (OoJae / Aperture) — **REPORTED**: the official privacy demo's Vercel
build bakes into its public JS bundle:

- `https://transaction-prover.alpha-sepolia.sw-dev.io`
- `https://discovery-service.alpha-sepolia.sw-dev.io`

"They answer unauthenticated with `access-control-allow-origin: *`, and we used the
Sepolia pair to prove our lifecycle end to end." The same team's repo
(`OoJae/aperture-strk20`, `.env.example`) commits both as working defaults, plus the
mainnet twins `transaction-prover.alpha-mainnet.sw-dev.io` /
`discovery-service.alpha-mainnet.sw-dev.io`. **REPORTED**.

### My live verification (harmless probes only, no proof submission)

```
curl -i https://transaction-prover.alpha-sepolia.sw-dev.io/            # GET
→ HTTP/2 405, body: "Used HTTP Method is not allowed. POST is required"
curl -i -X OPTIONS https://transaction-prover.alpha-sepolia.sw-dev.io/
→ HTTP/2 200, access-control-allow-methods: POST, access-control-allow-origin: *
curl -X POST .../ -d '{"jsonrpc":"2.0","id":1,"method":"starknet_specVersion","params":[]}'
→ {"jsonrpc":"2.0","id":1,"result":"0.10.3-rc.2"}
```

`starknet_specVersion` is exactly the SDK's own `isHealthy()` probe
(`sdk/src/internal/proving-service.ts:307`). **VERIFIED: the Sepolia prover is live,
unauthenticated, CORS-open.** The discovery service likewise: `GET /` → 404 (alive),
`GET /v1/sync/incoming_state` and `GET /v1/history` → **405** (paths exist, want POST).
**VERIFIED.** Bonus: `transaction-prover.alpha-mainnet.sw-dev.io` answers the same
specVersion probe (`0.10.3-rc.2`) — the "unpublished" mainnet prover responds too.
**VERIFIED** (relevant later for the ≥3-mainnet-tx question, not for this report's goal).

Nobody from StarkWare has answered #121/#124/#135/#147 in-thread about whether these
endpoints are sanctioned for external use. **VERIFIED (absence, as of today).**

## Q2 — Screening on Sepolia

- The Sepolia pool enforces screening: `get_screener_public_key` =
  `0x062f1e7c...51b552` (non-zero; established fact re-used, originally verified today).
- **How the attestation flows** (from `proof-interceptor/README.md` + SDK source at the
  pinned rev — **VERIFIED by source reading**): the prover calls its in-pod sidecar
  (`starknet_checkTransaction`) per prove request; a Deposit-carrying pool call is
  screened via elliptic-proxy, which on allow **also returns a STARK-curve signature over
  the depositor address**; the prover attaches it as `additional_data.signature` on the
  prove response; the SDK packs it as the trailing `Option<ScreeningAttestation>` in
  `apply_actions` calldata (`screening-calldata.ts`, `private-transfers.ts:buildExecuteResult`).
  The SDK's `pool-mode.ts` treats any class hash **not** pinned as pre-screening as
  screening-capable — the current Sepolia class `0x56ab118a...` is not pinned, so the
  suffix is packed automatically.
- **Is the hosted Sepolia prover minting attestations?** Chain evidence: I queried
  `starknet_getEvents` for the pool's `Deposit` key
  (`0x9149d2123147c5f43d258257fef0b7b969db78269369ebcf5ebb9eef8592f2`) over blocks
  14200024–14300024: **30 Deposit events, newest at block 14298384** (~today). The
  current class validates a screening attestation on every deposit, so every one of those
  carried a valid FPI signature. **VERIFIED** that attestations are being minted and
  accepted on Sepolia *by someone's prover*; **REPORTED** (Aperture) that specifically the
  SDK-route + `transaction-prover.alpha-sepolia` path shields successfully — their
  `cast-vote.ts` "shields 5 STRK publicly" headlessly on Sepolia, and their README
  documents Sepolia-specific failure modes of that exact path (fee 2 STRK from shielded
  balance, hangs on insufficient balance).
- **No document says testnet screening is permissive.** Nothing found in the monorepo
  docs, MAINNET-DAY-0, or the issues. Screening on Sepolia is the same fail-closed
  mechanism as mainnet (Elliptic address screening). A fresh faucet-funded address has no
  sanctions history; expected verdict is allow. **INFERRED.** Failure mode if wrong:
  JSON-RPC error `10000` ("Transaction rejected") or `screening_unavailable` (fail-closed
  signing path — `SCREENING_FAIL_OPEN` explicitly does not apply to it).

## Q3 — Self-hosted prover route

- **Image tags** (VERIFIED via GHCR API: anonymous pull token from `ghcr.io/token`, then
  `GET /v2/starkware-libs/starknet-privacy/transaction-prover/tags/list`): the PRIVACY
  line runs `PRIVACY-0.14.2-RC.1` … `RC.7`, **`PRIVACY-0.14.2-RC.8-screening-v2`**, then
  **`PRIVACY-0.14.3-RC.0`, `PRIVACY-0.14.3-RC.1`, `PRIVACY-0.14.3-RC.2`** (interleaved
  with unrelated APOLLO/test tags). So yes — **newer prover images than RC.2 exist**, and
  the `0.14.3-RC.x` line matches the SDK's `0.14.3-rc.*` versioning that pairs with the
  current (post-mainnet-v2) pool classes. Which exact tag matches deployed Sepolia class
  `0x56ab118a...` is not published; `PRIVACY-0.14.3-RC.2` is the newest candidate.
  **INFERRED** on the pairing; the earlier open question ("is there anything newer than
  RC.2?") is **answered: yes**.
- **Architectures**: both candidate tags are multi-arch **linux/amd64 + linux/arm64**
  (VERIFIED via manifest inspection) — runnable under Docker on this Apple Silicon
  machine, subject to the known cost: ~29 s/proof on 12 cores/46 GiB (REPORTED, upstream
  docs), 5–7 min on 2 vCPU (REPORTED, #147).
- **What it could do on Sepolia**: register, transfer, withdraw, `privacy_invoke` — every
  action whose screening the contract asserts to be `None`. **Not shield**: the
  attestation comes from the elliptic-proxy partner credentials that only StarkWare's
  deployment has (`SCREENING_PARTNER_NAME`/`SECRET`), and without `SCREENING_URL` the
  sidecar is a no-op that returns no signature — the deposit then reverts on-chain.
  REPORTED/INFERRED (interceptor README + contract behavior; consistent with
  `docs/notes/2026-08-30-consumer-path-discussion.md` §3, git history, removed
  2026-09-02).
- Given the hosted Sepolia prover is live and mints attestations, self-hosting is **not
  needed** for the goal of this report. It remains the fallback for everything except
  shield if the alpha endpoints get gated.

## Q4 — Funding (Sepolia STRK; ETH not needed)

**ETH is unnecessary.** The whole flow is v3 transactions, whose fees are
STRK-denominated; today's Sepolia (Starknet 0.14.x / RPC 0.10.2) rejects pre-v3
transactions outright — the node error I observed demands `version_0x3` +
full resource bounds (see Q5). **VERIFIED** (node validation error) + REPORTED.

### Starknet Foundation faucet — the headless path

`https://faucet.starknet.io` and `https://starknet-faucet.vercel.app` are the **same
Next.js deployment** (identical HTML, identical ETag `65a060e2...`). **VERIFIED.**

Two tiers (from the page and its JS chunk — **VERIFIED** by reading
`/_next/static/immutable/chunks/2m8je54pjmg1h.js`):

1. **Web UI**: "100 STRK · 24h" per address anonymous, Cloudflare **Turnstile captcha**
   (sitekey `0x4AAAAAAAyPTZS-7u4Crq4T`) + a server-issued hash bound to the address;
   a 3,000-STRK tier behind GitHub OAuth (`api.faucet.starknet.io/api/auth/github`).
   Not automatable (captcha; and bypassing captchas is off-limits anyway).
2. **Public Agent API** — the page's own words: "Fund a Starknet Sepolia address using
   the Starknet Faucet public Agent API. **No auth required** — requests are gated by
   proof-of-work, quotas, and cooldowns." Base URL `https://api.faucet.starknet.io`:

   ```
   1. POST /api/public-agent/pow/challenge          {"userAddress":"0x..."}
      → { data: { challengeId, salt, difficulty, powInputPrefix, expiresInSeconds } }
   2. solve locally: find nonce s.t. sha256(powInputPrefix + nonce)
      has `difficulty` leading zero BITS (bit-level, not hex-digit)
      powInputFormat: "challengeId:salt:userAddress:nonce"
   3. POST /api/public-agent/faucet/request         {"userAddress","challengeId","nonce"}
      → { data: { requestId, pollAfterSeconds } }
   4. GET  /api/public-agent/faucet/status/<requestId>
      → { data: { jobStatus, txHash } }   until "confirmed" | "failed"
   ```

   **VERIFIED live**: I requested one challenge (address `0x...01`, no funds requested):
   HTTP 201, `difficulty: 20`, `algorithm: "sha256-leading-zero-bits"`, expires 120 s.
   2^20 sha256 hashes is well under a second of local compute. Amount per grant is the
   same 100 STRK / 24 h quota as the anonymous UI tier (**INFERRED** from shared quota
   copy; not separately confirmed).

### Others

- **BlastAPI faucet** (`blastapi.io/faucets/starknet-sepolia-strk`): the page is a
  **service-deprecation notice** — faucet gone. **VERIFIED.**
- **Alchemy/Infura**: excluded by the task (accounts required).
- 100 STRK from one faucet grant covers the entire plan (account deploy ≪1 STRK,
  register+shield tx needs roughly 10 STRK deposit + 2 STRK pool fee + a gas-bounds
  *ceiling* observed around 6–9 STRK on Sepolia; issue #121 reports one team needed
  ~24 STRK total headroom for a registration to pass fee estimation). REPORTED numbers.

## Q5 — Account creation, headless

- **The installed `sncast` 0.34.0 does not work** against today's Sepolia. Probe (local
  account create against publicnode, nothing on-chain):
  `[WARNING] RPC node ... uses incompatible version 0.10.2. Expected version: 0.7.0`,
  then fee estimation fails — the node demands `version_0x3`, `resource_bounds`
  (all three), `tip`, `paymaster_data`, DA modes. **VERIFIED.** The installed `starkli`
  0.3.4 is the same era (its v3 support has l1-gas-only bounds, `--fee-token` defaults
  ETH) and the latest starkli release is 0.4.2 (2025-07-30) — predates RPC 0.10.
  **VERIFIED (release check) / INFERRED (0.4.2 incompatibility).**
- **Fix**: starknet-foundry **v0.63.0** (latest, 2026-08-06; available in
  `asdf list all starknet-foundry`). **VERIFIED.** Aperture ran this exact headless flow
  on Sepolia with sncast + publicnode (their README warns sncast rejects nodes below RPC
  0.10 — publicnode's bare host serves **0.10.2**, VERIFIED via `starknet_specVersion`).
- **Class**: sncast 0.63.0's default `oz` account is OpenZeppelin account **v1.0.0**,
  class hash `0x05b4b537eaa2399e3aa99c4e2e0208ebd6c71bc1467938cd52c798c601e43564`
  (constant in `crates/sncast/src/helpers/constants.rs` at tag v0.63.0 — VERIFIED), and
  that class **is declared on Sepolia** — `starknet_getClass` at latest returned its
  Sierra program. **VERIFIED.** (Same class Aperture uses for its ballot accounts —
  REPORTED.) No universal deployer involved: `account deploy` is a DEPLOY_ACCOUNT v3
  transaction, fee in STRK, paid by the pre-funded counterfactual address.
- **STRK-only**: the create→fund→deploy loop is v3 end to end; no ETH at any point.

## Q6 — The SDK, headless

- **Package**: `@starkware-libs/starknet-privacy-sdk`. The demo consumes it as
  `"starknet-sdk": "file:../sdk"`. Local checkout version **0.14.3-rc.2**; newest on the
  registry **0.14.3-rc.5** (REPORTED, #121). It is **not on npmjs** (`GET
  registry.npmjs.org/@starkware-libs%2fstarknet-privacy-sdk` → 404, **VERIFIED**);
  `publishConfig.registry` is **GitHub Packages** (`npm.pkg.github.com`), which requires
  *any* GitHub token with `read:packages` even for public packages. Working install
  (documented by Aperture, matches GH Packages rules):

  ```sh
  gh auth refresh -h github.com -s read:packages
  echo "//npm.pkg.github.com/:_authToken=$(gh auth token)" >> ~/.npmrc
  echo "@starkware-libs:registry=https://npm.pkg.github.com" >> ~/.npmrc
  ```

  Fallback needing no token: build from the source already on disk
  (`~/.cargo/git/checkouts/starknet-privacy-*/74841ca/sdk` → `npm install && npm run
  build`, consume via `file:`). Note rc.2 predates rc.5 fixes; prefer the registry.
- **Headless: yes.** Requirements: **Node ≥ 24** (`ohttp-ts` wants modern WebCrypto;
  fails opaquely on 22 — REPORTED twice in #121; this machine has **v26.6.0**,
  VERIFIED) and **starknet.js ≥ 10.4.0** (ships on the npm `next` tag; carries the
  proof-aware `execute` and STRK20 wallet types — REPORTED). The e2e tests and two
  hackathon backends (Aperture tally service, Shoal) drive it from plain Node with a raw
  private key. No browser, no wallet extension.
- **Wiring** (VERIFIED from `sdk/src/factory.ts`, `interfaces.ts`, e2e smoke test):

  ```ts
  const transfers = createPrivateTransfers({
    account,                                  // starknet.js Account(provider, addr, privKey)
    viewingKeyProvider: { getViewingKey: async () => viewingKey },
    provingProvider:   { url: PROVER_URL,  chainId: constants.StarknetChainId.SN_SEPOLIA },
    discoveryProvider: { url: INDEXER_URL },
    poolContractAddress: POOL,
  });
  const { callAndProof } = await transfers.build({ autoRegister: true, autoSetup: true })
    .with(STRK).deposit({ amount })          // .transfer({recipient, amount}) / .withdraw(...)
    .surplusTo(account.address)
    .execute();                              // ← calls the proving service
  await account.execute(callAndProof.call,   // apply_actions(+ screening suffix)
    { tip: 0n, proof: callAndProof.proof.data, proofFacts: callAndProof.proof.proofFacts });
  ```

  The proof rides **in the transaction itself** (`tx_info.proof_facts` on-chain;
  starknet.js `execute` options `proof`/`proofFacts`) — submitted through the ordinary
  RPC node, which is how Aperture submitted via publicnode. Viewing-key derivation
  (canonical, wallet-compatible): sign `starknetKeccak("${chainId}:${poolAddress}")`,
  Poseidon-fold `(r,s)`, reduce mod curve order (MAINNET-DAY-0 + demo README).
- **Prover API** (VERIFIED from `sdk/src/internal/proving-service.ts`): plain JSON-RPC
  POST to the base URL. `starknet_specVersion` (health);
  `starknet_proveTransaction { block_id, transaction }` where `transaction` is an
  **INVOKE_TXN_V3** wrapping one `Call` to the pool's `compile_actions(user_addr,
  user_private_key, client_actions)` (nonce hardcoded 0, skipValidate, signed by the
  account signer). Result `{ proof: base64, proof_facts: [...], l2_to_l1_messages,
  additional_data?: { signature? } }`. Error codes: `-32005` busy (retried),
  `10000` screening-rejected, `55` account validation, `61` unsupported tx version,
  `1000` invalid input. Note what this implies: **the account private key and the pool
  viewing key are both inside the prove request** — use throwaway Sepolia keys.
- **Hand-rolling without the SDK is not realistic.** The pool class (VERIFIED via
  `starknet_getClass` on `0x56ab118a...`) exposes exactly three non-admin externals:
  `__execute__`, `compile_and_panic`, `apply_actions(Span<ServerAction>,
  Option<ScreeningAttestation>)`. The `ServerAction` span must be the prover's own
  output (`proof.output` minus the class-hash prefix) and the proof must accompany the
  tx; client-side you'd be reimplementing action serialization, note encryption and
  channel derivation for zero benefit. The realistic minimal client is: SDK for
  compile+prove+calldata, starknet.js for submission — which is what the recipe uses.

---

## Decision-ready recipe (NOT yet executed)

Target: one transaction on Sepolia emitting `ViewingKeySet` + `Deposit` +
`EncNoteCreated` (registration and shield batch into a single `apply_actions`, exactly
like our own mainnet tx `0x40093f...d19d`), then optionally a second, private
note-to-note transfer for spent-state testing.

```sh
# ── 0. toolchain (one-time) ────────────────────────────────────────────────
asdf install starknet-foundry 0.63.0 && asdf set starknet-foundry 0.63.0   # local sncast 0.34 is RPC-0.7 era and FAILS (verified)
gh auth refresh -h github.com -s read:packages                              # [RISK R6] interactive browser step
echo "//npm.pkg.github.com/:_authToken=$(gh auth token)" >> ~/.npmrc
echo "@starkware-libs:registry=https://npm.pkg.github.com" >> ~/.npmrc

# ── 1. account keypair + counterfactual address (offline except a chain-id read) ──
RPC=https://starknet-sepolia-rpc.publicnode.com
sncast account create -n strk20-sepolia --url $RPC        # prints ADDRESS; keys in ~/.starknet_accounts

# ── 2. fund via the faucet Agent API (headless, PoW, no captcha) ──────────
# challenge → solve sha256 leading-zero-bits (difficulty 20 ≈ <1 s) → request → poll
curl -s -X POST https://api.faucet.starknet.io/api/public-agent/pow/challenge \
     -H 'Content-Type: application/json' -d "{\"userAddress\":\"$ADDRESS\"}"
# solve: nonce s.t. sha256("challengeId:salt:userAddress:" + nonce) has 20 leading zero bits
curl -s -X POST https://api.faucet.starknet.io/api/public-agent/faucet/request \
     -H 'Content-Type: application/json' \
     -d "{\"userAddress\":\"$ADDRESS\",\"challengeId\":\"$CID\",\"nonce\":\"$NONCE\"}"
# poll /api/public-agent/faucet/status/<requestId> until confirmed  → 100 STRK  [RISK R5]

# ── 3. deploy the account (DEPLOY_ACCOUNT v3, STRK fee) ───────────────────
sncast account deploy -n strk20-sepolia --url $RPC        # OZ v1.0.0 class, declared on Sepolia (verified)

# ── 4. Node project ───────────────────────────────────────────────────────
npm init -y && npm i @starkware-libs/starknet-privacy-sdk@0.14.3-rc.5 starknet@next
# Node >= 24 required (have 26.6.0); starknet >= 10.4.0 required for proof-carrying execute

# ── 5. one script (sketch — the load-bearing calls) ───────────────────────
#   provider = RpcProvider({ nodeUrl: RPC })              # publicnode: no 'pending' tag [RISK R4]
#   account  = Account(provider, ADDRESS, PRIVATE_KEY)
#   viewingKey = poseidon(r,s) % curve.n  over  sign(starknetKeccak("0x534e5f5345504f4c4941:" + POOL))
#   await account.execute({ contractAddress: STRK, entrypoint: "approve",
#                           calldata: [POOL, 10e18, 0] })            # [RISK R3] mandatory
#   transfers = createPrivateTransfers({ account,
#       viewingKeyProvider: { getViewingKey: async () => viewingKey },
#       provingProvider:   { url: "https://transaction-prover.alpha-sepolia.sw-dev.io",
#                            chainId: SN_SEPOLIA, requestTimeoutMs: 120_000 },   # [RISK R2]
#       discoveryProvider: { url: "https://discovery-service.alpha-sepolia.sw-dev.io" },
#       poolContractAddress: POOL })
#   { callAndProof } = await transfers.build({ autoRegister: true, autoSetup: true })
#         .with(STRK).deposit({ amount: 10n * 10n**18n }).execute()  # register+shield, ONE tx  [RISK R1]
#   await account.execute(callAndProof.call,
#         { tip: 0n, proof: callAndProof.proof.data, proofFacts: callAndProof.proof.proofFacts })
#   → receipt carries ViewingKeySet + Deposit + EncNoteCreated       # ← the goal
#
# constants: POOL = 0x0254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91
#            STRK = 0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d

# ── 6. optional: private transfer for spent-state (2nd registered key needed) ──
#   wait ≥10 blocks (note maturity ≈ 17 s), then
#   .with(STRK).transfer({ recipient: SECOND_ADDR, amount: ... }).surplusTo(ADDRESS).execute()
#   fee: 2 STRK from the SHIELDED balance per pool tx — shield enough up front
```

### Risk flags

- **R1 — endpoint legitimacy/stability (the only real one).** The alpha-sepolia pair is
  StarkWare infra, publicly reachable and third-party-used, but never explicitly
  sanctioned; #121 asked and no answer is on record. It can be gated or revoked at any
  time. Mitigation: it's testnet, cost of a dead end is near zero; self-hosted prover
  (`PRIVACY-0.14.3-RC.2`, arm64 available) covers register/transfer/withdraw as fallback
  — only shield is hosted-only. Screening of a fresh faucet address should pass
  (Elliptic address screening); failure surfaces as JSON-RPC `10000`.
- **R2 — timeouts.** Proving takes ~30 s; the SDK's default request timeout is exactly
  30 s → set `requestTimeoutMs` generously. Known trap (three days lost by another team,
  twice reported): **insufficient shielded balance presents as a proving hang or an
  opaque 500**, not an error naming the cause. Proof validity is 450 blocks ≈ 13 min at
  Sepolia's ~1.7 s/block — submit promptly after proving; you cannot pre-prove.
- **R3 — ERC20 allowance.** Nothing approves for you server-side. Without a prior
  `approve(pool, amount)` the failure appears only at fee estimation as
  `Insufficient ERC20 allowance` buried in a full transaction dump.
- **R4 — RPC quirks.** publicnode has no `pending` tag (use `latest`); it does serve
  spec 0.10.2 on the bare host, which new sncast requires.
- **R5 — gas-bounds ceiling.** A pool tx wants a resource-bounds *ceiling* far above the
  bill (~5.8–9 STRK observed for a registration on Sepolia; one team needed ~24 STRK
  total balance before estimation passed). With the 100 STRK faucet grant this is a
  non-issue; don't try to run it at 15 STRK.
- **R6 — npm auth.** GitHub Packages needs the `read:packages` scope the current `gh`
  token lacks (verified: 403 on the packages API). One interactive `gh auth refresh`, or
  build the SDK from the local pinned checkout (rc.2 — older than rc.5).

### What this does NOT change

`docs/roadmap.md` cut the write path from the product deliberately, and nothing here
argues otherwise. This is a **development capability**: our own transactions on Sepolia
to exercise the read path (spent-state detection, subscription confirmation) end to end
— roadmap item 6's "test wallets/keys" made concrete.

---

## Evidence log (exact probes)

| # | Action | Result |
|---|--------|--------|
| 1 | Read `demo/.env.example`, `.env.mainnet.example`, `demo/README.md`, `proof-interceptor/README.md`, `sdk/src/**` at local pinned checkout `74841ca` | as cited above |
| 2 | `GET api.github.com/repos/starkware-libs/starknet-privacy/commits` | HEAD `f6cabbef` 2026-08-30 |
| 3 | `GET raw.githubusercontent.com/.../main/demo/.env.example`, `.env.mainnet.example` | unchanged; mainnet prover TODO |
| 4 | `gh issue view` starkience/strk20-hackathon **#121, #124, #135, #147, #223, #31** (+ all comments) | endpoints, SDK version rc.5, gas numbers, register-needs-proof |
| 5 | `GET/OPTIONS/POST(specVersion) transaction-prover.alpha-sepolia.sw-dev.io` | 405 / 200 / `"0.10.3-rc.2"` |
| 6 | `POST(specVersion) transaction-prover.alpha-mainnet.sw-dev.io` | `"0.10.3-rc.2"` |
| 7 | `GET discovery-service.alpha-sepolia.sw-dev.io{/, /v1/sync/incoming_state, /v1/history}` | 404 / 405 / 405 |
| 8 | GHCR token + `GET /v2/.../transaction-prover/tags/list` + 2 manifests | PRIVACY tags through `0.14.3-RC.2`; amd64+arm64 |
| 9 | `GET registry.npmjs.org/@starkware-libs%2fstarknet-privacy-sdk` | 404 |
| 10 | `starknet_getClass(0x56ab118a...)` on publicnode Sepolia | full ABI; 3 non-admin externals |
| 11 | `starknet_getEvents` pool Deposit key, blocks 14200024–14300024 | 30 events, newest 14298384 |
| 12 | `starknet_getTransactionByHash` on 2 recent deposits | INVOKE v3, approve+deposit shape |
| 13 | `starknet_specVersion` publicnode bare host | `0.10.2` |
| 14 | `sncast 0.34.0 account create` probe vs publicnode | incompat 0.7 vs 0.10.2; node demands v3 fields |
| 15 | `GET raw .../foundry-rs/starknet-foundry/v0.63.0/.../constants.rs`; `starknet_getClass(0x05b4b537...)` | OZ v1.0.0 hash; declared on Sepolia |
| 16 | `faucet.starknet.io` HTML + JS chunk `2m8je54pjmg1h.js`; ETag match with starknet-faucet.vercel.app | tiers, Turnstile sitekey, full Agent API text |
| 17 | `POST api.faucet.starknet.io/api/public-agent/pow/challenge` (addr `0x…01`) | 201; difficulty 20, sha256-leading-zero-bits, 120 s expiry |
| 18 | `GET blastapi.io/faucets/starknet-sepolia-strk` | deprecation notice |
| 19 | `OoJae/aperture-strk20` `.env.example` + README (raw fetches) | committed endpoint defaults; full headless Sepolia runbook |
| 20 | Local: `node --version` (26.6.0), `starkli 0.3.4 --help`, `sncast 0.34.0`, `docker 27.4.0`, `asdf list all starknet-foundry` (has 0.63.0) | as cited |
