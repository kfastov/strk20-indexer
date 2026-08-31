# Sepolia shield run — our own `EncNoteCreated`, executed

> Two runs. **Run 1** (§1–§8): register + shield under pool class `0x56ab118a…`.
> **Run 2** (§"Run 2"): spend that note under the post-upgrade class `0x7e2bbd7c…`.

Run date: 2026-08-31. Goal: produce **our own note** on the Sepolia STRK20 privacy pool
(`0x0254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91`), headless, so the
indexer can discover it keylessly end to end. Testnet only; no mainnet write path was
touched (the script hard-aborts unless `starknet_chainId == SN_SEPOLIA`).

Companion to `sepolia-write-path.md` (the pre-flight research). That report's verdict —
"the chain is unbroken" — held. Its recipe needed eleven corrections (C1–C11), listed in
§"Corrections" and marked inline.

Evidence discipline: **OBSERVED** = a command in this run produced it. **INFERRED** =
follows from observed facts but was not directly measured.

## Result

**The note exists on chain.** OBSERVED:

| | |
|---|---|
| transaction | `0x701e056354f9e0e17e86b7d63d4403cb46e239e7061806e9f5e02ff47d65f49` |
| block | **14,339,115**, tx_index 0, block hash `0x8b5cd12c4f89228b272a4a8bc3671d11dea8184da489ff88c3e1688fc6b347` |
| timestamp | 2026-08-31T14:20:35Z |
| status | `SUCCEEDED` / `ACCEPTED_ON_L2` |
| fee paid | 3.026531 STRK (`0x2a006618ea011460` FRI) |
| depositor | `0x370b9bc1cbb37bf295f64845fe78d57f4fbb95057332278776ea3b6505c6408` |
| note id | `0x00ce526b286fed962b9e3942771c5e519c69b8677dc24136ae380ba523a067ff` |
| note amount | 3.000000 STRK, unspent |

All three target events, from the pool address, in one `apply_actions` — registration and
shield batched exactly as intended:

```
EncNoteCreated  0x023c20207be8b1ef4430c25eef8ce779c9745ebe04139555ae81bd4f8fdd6ec5
  keys: [selector, 0x00ce526b286fed962b9e3942771c5e519c69b8677dc24136ae380ba523a067ff]   # note id
  data: [0x00cb93562e444a2f62b371b88d6be6350d7a019df3f3455fe4325ed78c514bb4]              # packed enc value
ViewingKeySet   0x01321a492485b4f19851fb787ab3800a0030b595332cba93cd5fe40dfb5a4daf
  keys: [selector, 0x370b9bc1…c6408 (user), 0x7672edb34c78308d4601da1dbf892d11cb3f8311cb002e5583a15fc93c636c4 (pool pubkey)]
  data: [0x01d17f98be07e99713265714699a5c40ccbf7b50c950fb7a2abd81846fcdfbb2,
         0x0165f765b6771bf17b916e3d99c977eeec6732e88a4e9b57673ecd4de70e5d7e,
         0x03597292731e7919d2905a6fc3a4df7fce046a5c82629666530f3d6834c9a924]              # EncPrivateKey
Deposit         0x009149d2123147c5f43d258257fef0b7b969db78269369ebcf5ebb9eef8592f2
  keys: [selector, 0x370b9bc1…c6408 (depositor), 0x04718f5a…c938d (STRK)]
  data: [0x29a2241af62c0000]                                                              # 3e18 = 3 STRK
```

Independent confirmations (`verify.mjs`), all OBSERVED:

- `get_public_key(0x370b9bc1…c6408)` on the pool returns
  `0x7672edb34c78308d4601da1dbf892d11cb3f8311cb002e5583a15fc93c636c4`, which **equals**
  `starkKey(viewingKey)` recomputed from the saved key file. The registration is real and
  the saved viewing key is the one the pool knows.
- `transfers.discoverNotes()` with the saved viewing key returns exactly **1 STRK note,
  3.000000 STRK, created 14339115, open=false, sender = our address**. The note is
  discoverable by the key we persisted — which is what the indexer test needs.

### Where the key material lives

`/Users/konstantinfastov/Projects/strk20-indexer/data/sepolia/viewing-key-strk20test.json`
— mode `0600`, under the gitignored `data/` tree (`git check-ignore` confirms
`.gitignore:4:data/`). JSON, one object:

```jsonc
{
  "network": "sepolia", "chain_id": "0x534e5f5345504f4c4941",
  "rpc": "https://starknet-sepolia-rpc.publicnode.com",
  "pool_address": "0x0254a6b2…e0d91", "strk_token": "0x04718f5a…c938d",
  "account_name": "strk20test",
  "account_address": "0x370b9bc1cbb37bf295f64845fe78d57f4fbb95057332278776ea3b6505c6408",
  "account_public_key": "0x0c28376c…21987",
  "account_private_key": "0x…",          // SECRET — testnet throwaway
  "viewing_key": "0x344089…210c",        // SECRET — 0x-hex felt, 65 chars
  "viewing_key_decimal": "…",            // same value, base 10
  "derivation": { "message": "<chainId>:<pool>", "scheme": "…", "sdk_version": "0.14.3-rc.5" },
  "pool_public_key_onchain": "0x7672edb3…636c4",
  "transactions": { "approve": {…}, "apply_actions": { "hash", "block", "block_hash",
                    "tx_index", "fee_fri", "deposit_amount", "pool_fee", "proving_block",
                    "events_observed" } }
}
```

The viewing key is *derived*, not random: `starknetKeccak("<chainId>:<pool>")` → Stark
ECDSA sign with the account key (RFC-6979, deterministic) → `poseidon([r,s])` → mod curve
order → canonicalise into the lower half. Identical to upstream `demo/src/session.ts`, so
anyone holding `account_private_key` recomputes the same viewing key. The file also
carries it verbatim so the indexer test does not have to redo the derivation.

Also written (gitignored, working artifacts): `data/sepolia/shield/` holds the scratch
Node project — `lib.mjs`, `status.mjs`, `shield.mjs`, `verify.mjs`, `sweep.mjs`,
`survey-fees.mjs`, `decode-tx.mjs`, `last-callandproof.json` (the exact
`apply_actions` calldata + screening attestation + proof facts of the successful run),
`run2.log` / `run3.log`, and `upstream/` (the cloned SDK source at tag
`PRIVACY-0.14.3-RC.5`).

## SDK: which version, and how it was obtained

**The GitHub Packages route is blocked on this machine, and it did not matter.** OBSERVED:

```
$ gh auth status → Token scopes: 'admin:public_key', 'gist', 'read:org', 'repo'   # no read:packages
$ npm view @starkware-libs/starknet-privacy-sdk versions
npm error code E403
npm error 403 Forbidden - GET https://npm.pkg.github.com/@starkware-libs%2fstarknet-privacy-sdk
      - Permission permission_denied: The token provided does not match expected scopes.
```

The local `.npmrc` written for the attempt was deleted immediately afterwards; the token
was never printed, never committed, and is not on disk anywhere now.

The unblock: **`starkware-libs/starknet-privacy` is a public Apache-2.0 repo**
(`gh api repos/… → "private": false`), and `sdk/` builds standalone with no registry auth:

```sh
git clone https://github.com/starkware-libs/starknet-privacy.git upstream
git -C upstream checkout PRIVACY-0.14.3-RC.5
cd upstream/sdk && npm ci && npm run build      # tsc -p tsconfig.build.json → dist/
```

Consumed from a sibling `package.json` as
`"@starkware-libs/starknet-privacy-sdk": "file:./upstream/sdk"` alongside
`"starknet": "10.5.0"`. Loads and runs in plain Node **v26.6.0**, no browser, no OHTTP.

### Picking the version empirically (this mattered)

Rather than guess "rc.4-era", the deployed class's own ABI was diffed against
`sdk/src/internal/abi.ts` at each candidate tag (script: `data/sepolia/shield/cmp-abi.mjs`,
canonicalising every function / struct / enum / event). OBSERVED:

| SDK tag | date | entries on-chain / in SDK | diffs |
|---|---|---|---|
| PRIVACY-0.14.3-RC.2 | 2026-07-01 | 117 / 115 | **5** — no `ComputeAndInvokeInput`, `ClientAction` and `ServerAction` both short a variant, no `ExternalContractInvoked` |
| PRIVACY-0.14.3-RC.3 | 2026-07-08 | 117 / 117 | **0** |
| PRIVACY-0.14.3-RC.4 | 2026-07-22 | 117 / 117 | **0** |
| **PRIVACY-0.14.3-RC.5** | 2026-08-12 | 117 / 117 | **0** ← used |
| PRIVACY-0.14.3-RC.6 | 2026-08-31 (today) | 117 / 118 | **8** — swaps the open-note depositor block list for `OpenNoteScreeningPolicy` |

The Sepolia class `0x56ab118a…45623b2` was activated at block 12,932,675 =
**2026-08-04T11:07:29Z** (OBSERVED), i.e. between RC.4 and RC.5 — consistent with the
three-tag exact-match band. RC.5 chosen as the newest exact match.

This is load-bearing: **the fallback the research report proposed (build from the local
pinned checkout at rev `74841ca`, version 0.14.3-rc.2) would have shipped the wrong
`ClientAction`/`ServerAction` enum layout** — a silent serialization mismatch, not a clean
error. And RC.6, released hours before this run, diverges the other way.

### API actually used

```js
import { createPrivateTransfers, ProvingServiceProofProvider,
         IndexerDiscoveryProvider, MAX_VIEWING_KEY } from "@starkware-libs/starknet-privacy-sdk";
import { RpcProvider, Account, constants, cairo, hash, ec } from "starknet"; // 10.5.0

const provider = new RpcProvider({ nodeUrl: RPC, batch: false });
const account  = new Account({ provider, address, signer: privateKey, cairoVersion: "1" });

const transfers = createPrivateTransfers({
  account,
  viewingKeyProvider: { getViewingKey: async () => viewingKey },
  provingProvider:  new ProvingServiceProofProvider(
      "https://transaction-prover.alpha-sepolia.sw-dev.io",
      constants.StarknetChainId.SN_SEPOLIA, { requestTimeoutMs: 180_000 }),
  discoveryProvider: new IndexerDiscoveryProvider(
      "https://discovery-service.alpha-sepolia.sw-dev.io", POOL),   // no options ⇒ OHTTP off
  poolContractAddress: POOL,
});

const provingBlockId = (await provider.getBlockNumber()) - 9;      // see correction C7
const chain = transfers
  .build({ autoRegister: true, autoSetup: true,
           autoDiscover: { notes: "refresh", channels: "refresh" }, autoSelectNotes: "naive" })
  .surplusTo(address)
  .with(STRK, (t) => { t.deposit({ amount: 3n * 10n ** 18n }); });

const invocation = await chain.createProofInvocation({ provingBlockId });
const { callAndProof } = await transfers.executeWithInvocation(invocation, provingBlockId);
await account.execute(callAndProof.call,
  { tip: 0n, proof: callAndProof.proof.data, proofFacts: callAndProof.proof.proofFacts });
```

This is the upstream demo's own direct-execution path
(`demo/src/hooks/useTransactionBuilder.ts` + `useTransactions.ts`), minus the paymaster
branch. `createPrivateTransfers` also accepts plain configs (`{url, chainId}` /`{url}`) and
builds the same two providers with OHTTP off — either form works.

Prove-side facts OBSERVED for the winning run: **3.7 s** round trip, proof **218,778
bytes** base64-decoded, **9 proof facts**, 56 server-action felts in the L2→L1 payload,
59-felt `apply_actions` calldata, and a **screening attestation present**:
`additional_data.signature = { issued_at: 1788186029, sig_r: 0x07ec71f0…c5f0, sig_s: 0x029a99a4…e217 }`.
The proof invocation itself is an INVOKE_TXN_V3 from the *pool* address with
`PROOF_INVOCATION_NONCE = 0n`, zero resource bounds and query version `0x1…03`, exactly as
the research report described.

## Budget: what it actually cost

Measured **before** committing, by surveying the 30 real `Deposit` events in the last
40,000 Sepolia blocks (`survey-fees.mjs`) — OBSERVED across 8 recent pool transactions:
gas actually paid **2.92–3.12 STRK**, resource-bounds **ceiling 7.85–8.39 STRK**. That
sized the funding round.

This run's own numbers, OBSERVED:

| step | tx | fee (STRK) |
|---|---|---|
| faucet grant → fund1 | `0x600509813574eab4254fd76d8c2e99f304c01073b23a13a41fb7e1114cdf702` | +5.000000 |
| faucet grant → fund2 | `0x242e04ddfdcf37eeb93c9133821a53596d8ef639c69e12408130032b3d27c53` | +5.000000 |
| `account deploy` fund1 | `0x058c49198d80d7d06432311274aabcf9eb5b41b9a7b5e930667fce131396223c` | 0.078564 |
| `account deploy` fund2 | `0x00c509544246008d6c0d394cca844e884bb4db15d72adf0760f380926c219c83` | 0.078564 |
| sweep fund1 → main (4.775299) | `0x19d69b0fad19866ac739df10d34bf2b06320f6fc82e0130944561e90e02ddb4` | 0.049239 |
| sweep fund2 → main (4.775299) | `0x25859905f28830f13ff5ba042c8d430e060decda61133105eacfbaed7cc1c70` | 0.049239 |
| `approve(pool, 5 STRK)` | `0x12bddac7a4dda4bec124bbc91fc30122a88eecfc6cbcd5fdb1f99df0f5372f2` (block 14,339,047) | 0.054584 |
| **`apply_actions`** | **`0x701e056354f9e0e17e86b7d63d4403cb46e239e7061806e9f5e02ff47d65f49`** (block 14,339,115) | **3.026531** |

`apply_actions` pre-submission estimate: **overall 6.796413 STRK, bounds ceiling 6.796413
STRK** — actual 3.026531, i.e. the estimate is ~2.2× the bill, and the *ceiling* is the
number the balance must clear. Main account 14.417615 → 6.391084 STRK, so the transaction
consumed exactly **3 (deposit) + 2 (pool fee) + 3.026531 (gas) = 8.026531 STRK**.

Faucet draw: **10 STRK total across two new addresses** (the two additional accounts the
task permitted; `strk20test` was already on its 24 h cooldown). Residue left stranded:
0.096898 STRK on each helper.

## Every command, in order

```sh
# ── SDK (no registry auth) ────────────────────────────────────────────────────
gh auth status                                            # → no read:packages  [C1]
npm view @starkware-libs/starknet-privacy-sdk versions     # → E403  [C1]
git clone https://github.com/starkware-libs/starknet-privacy.git data/sepolia/shield/upstream
gh api repos/starkware-libs/starknet-privacy/tags --paginate --jq '.[].name'
node cmp_abi.mjs                                           # ABI-diff every tag vs on-chain class  [C2]
git -C upstream checkout PRIVACY-0.14.3-RC.5
(cd upstream/sdk && npm ci && npm run build)
npm install                                                # file:./upstream/sdk + starknet@10.5.0

# ── budget survey (read-only) ─────────────────────────────────────────────────
node status.mjs            # chain id, pool version/fee/screener/paused, balances, prover+discovery health
node survey-fees.mjs       # 30 real Deposits, fees paid + bounds ceilings
node decode-tx.mjs <hash>  # value-flow decode of three reference deposits  [C3]

# ── funding ───────────────────────────────────────────────────────────────────
ASDF_STARKNET_FOUNDRY_VERSION=0.63.0 asdf exec sncast \
  --accounts-file data/sepolia/accounts.json account create -n fund1 --url $RPC
#   … same for fund2
python3 data/sepolia/shield/faucet/flow4.py 0x064a9fea…16aa9 10      # keep-alive-pinned PoW flow  [C6]
python3 data/sepolia/shield/faucet/flow4.py 0x04815437…9cf7b 10
ASDF_STARKNET_FOUNDRY_VERSION=0.63.0 asdf exec sncast \
  --accounts-file data/sepolia/accounts.json account deploy --url $RPC --name fund1
#   … same for fund2
node sweep.mjs                                   # estimateInvokeFee → transfer residue to strk20test  [C10]

# ── the run ───────────────────────────────────────────────────────────────────
node save-keys.mjs strk20test                    # derive + persist viewing key BEFORE any tx
node shield.mjs                                  # approve → prove → estimate → apply_actions
node verify.mjs                                  # receipt, 3 events, get_public_key, discoverNotes
```

`shield.mjs` is idempotent-ish: it skips `approve` when the allowance already covers
`deposit + fee`, and it aborts before submitting if fee estimation fails.

## Corrections to `sepolia-write-path.md`

**C1 — R6 ("npm auth") is real but not a blocker, and the stated fallback was wrong.**
The 403 is confirmed exactly as predicted. But the report offers only "build from the
local pinned checkout (rc.2 — older than rc.5)". The repo is **public**: cloning it gives
*every* tag with no token at all. `gh auth refresh -s read:packages` was never needed.

**C2 — SDK version can be pinned by ABI-matching, and rc.2 would have been wrong.** See
the table above. RC.3/4/5 match the deployed class byte-for-byte on all 117 ABI entries;
rc.2 (the local checkout) is missing the `ComputeAndInvoke` variant in both action enums.
This is a cheap, decisive check that should precede any future run — including after the
next pool upgrade, since RC.6 (released the same day) already diverges.

**C3 — the 2 STRK pool fee is charged from the caller's *public* STRK balance, not from
the shielded note.** `Privacy::collect_fee` (packages/privacy/src/privacy.cairo:841) does
`checked_transfer_from(STRK_TOKEN_ADDRESS, sender: get_caller_address(), recipient:
fee_collector, amount: fee_amount)` **before** applying actions. Two consequences the
report gets wrong:

  1. **The ERC20 `approve` must cover `deposit + fee`, not just `deposit`.** Approving only
     the deposit amount — which is what the upstream demo's own direct branch does
     (`useTransactionBuilder.ts` approves `totalAmount` per token) — reverts in
     `collect_fee`. We approved 5 STRK for a 3 STRK deposit. This makes R3 stricter, not
     just "mandatory".
  2. **The note is the full deposit.** 3 STRK deposited → a 3.000000 STRK note (verified by
     `discoverNotes`). The report's "on mainnet a 10 STRK deposit with a 6 STRK fee left
     ~4 shielded" describes the *paymaster* flow under the older class: every recent
     Sepolia deposit routed through relayer `0x75a180e1…` shows the user pre-paying 2 STRK
     to the relayer, the relayer paying `collect_fee`, and the SDK adding a **shielded
     `Withdrawal` of 2 STRK** to reimburse it. Direct execution has no such Withdrawal —
     our receipt has exactly three pool events, no `Withdrawal`.

**C4 — gas headroom: ~7–8.4 STRK ceiling, not ~24.** R5 cites "one team needed ~24 STRK
total balance before estimation passed". OBSERVED: estimate ceiling 6.796413 STRK, and
14.417615 STRK of balance was ample with room to spare. Plan for **~10 STRK free +
deposit**, not 24.

**C5 — the faucet grant is 5 STRK per address, not 100.** Q4 infers 100 STRK/24 h from the
web UI's quota copy. Both public-agent grants in this run credited exactly 5.000000 STRK.
Cooldown is per address and did apply (`429 ADDRESS_COOLDOWN`).

**C6 — the faucet's `POW_CHALLENGE_INVALID` is intermittent, and connection pinning fixes
it.** The documented body (`{userAddress, challengeId, nonce, network:"sepolia"}`) is
correct but **not sufficient**: with a fresh address and a freshly solved, verified
20-bit PoW, five separate submissions were rejected with
`400 POW_CHALLENGE_INVALID "Proof-of-work challenge does not match this address or
network."` and one succeeded — under identical bodies (address padding, string-vs-number
nonce, and a settling delay were each ruled out as the discriminator, though a
number-typed nonce happened to be the one that landed). Reusing **one keep-alive TCP
connection across `/pow/challenge` and `/faucet/request`** succeeded on the first attempt.
INFERRED: the API is load-balanced across instances that do not share challenge storage.
Working script: `data/sepolia/shield/faucet/flow4.py` (single `http.client.HTTPSConnection`, bounded
retries, exits on `ADDRESS_COOLDOWN`). Difficulty was 20 bits, solved in 0.4–1.2 s.

**C7 — new, undocumented in the report: the proving block must be old enough.** Proving at
`latest` (the SDK's own default `blockIdentifier`) fails at fee estimation with

```
41: Transaction execution error: {"transaction_index":0,"execution_error":
    "Invalid proof facts: The proof block number 14339075 is too recent.
     The maximum allowed block number is 14339068."}
```

— i.e. head−7 at that moment. Upstream handles this with `PROVING_BLOCK_DEPTH = 9` in
`demo/src/hooks/useTransactions.ts` (`waitForProvingBlock`, which additionally waits when
the previous tx is newer than head−9). Using `head − 9` succeeded first try. This cost one
wasted proof and is the single failure this run actually hit. The Cairo side confirms both
ends of the window: `assert(base_block_number < current_block_number)` and
`current_block_number <= base_block_number + proof_validity_blocks` with
`get_proof_validity_blocks() = 450` (OBSERVED on-chain).

**C8 — proving is fast: 3.6–3.7 s, not ~30 s.** R2 sizes timeouts against a ~30 s proof and
warns the SDK's 30 s default is marginal. On this hosted Sepolia prover a
register+deposit proved in under 4 s twice. The 450-block validity window is real, but
there is no timeout pressure for a single deposit. (We still set
`requestTimeoutMs: 180_000`.)

**C9 — screening passed, first try, for a fresh faucet-funded address.** R1's failure mode
(`10000` / `screening_unavailable`) did not occur. The prove response carried
`additional_data.signature`, the SDK packed the `Option<ScreeningAttestation>` suffix, and
the contract accepted it. Q2's inference is now OBSERVED for our own address.

**C10 — starknet.js 10.5 API details the report's sketch gets wrong.**
`account.estimateFee` does not exist — it is **`estimateInvokeFee`**. The `Account`
constructor is object-form: `new Account({ provider, address, signer, cairoVersion })`.
Both `proof` and `proofFacts` are accepted in `UniversalDetails`, so
`estimateInvokeFee(calls, { tip: 0n, proof, proofFacts })` gives a real pre-submission
estimate (the report only shows them on `execute`).

**C11 — RPC block-id shape on publicnode.** `starknet_getClass` takes the block id as a
bare tag string: `params: ["latest", classHash]`. The object form `[{block_id:"latest"}, …]`
is rejected with `-32602 "cannot unmarshal block id"`. (Adds to R4's `pending` note.)

## Confirmed unchanged from the research report

OBSERVED in this run: the hosted Sepolia prover answers `starknet_specVersion` →
`"0.10.3-rc.2"` and proves for us unauthenticated; the discovery service's `/health`
returns `{"status":"OK", …, "lag_secs": 6}`; publicnode serves spec **0.10.2** and chain id
`SN_SEPOLIA`; the pool reports version `3288624` (= `"2.0"` short string), fee
`0x1bc16d674ec80000` (2 STRK), collector `0x03e6c6f41…e4766`, screener key
`0x062f1e7ca…1b552` (non-zero), `is_paused = false`; sncast **0.63.0** works and 0.34.0
does not; Node 26.6.0 runs the SDK headless.

The R1 policy caveat is unchanged: the alpha-sepolia endpoints answered us happily, but
nobody from StarkWare has sanctioned them. This run consumed one register+shield of
proving capacity.

## What this unlocks, and what is still open

The indexer can now be pointed at Sepolia with a key we control and asked to find note
`0x00ce526b…67ff` at block 14,339,115 by keyless discovery — the first end-to-end test
where we own both sides. Registration state (`ViewingKeySet`), a deposit, and a note
creation all land in one transaction, so a single block exercises three event paths.

Not done at the time of Run 1, and cheap follow-ups if wanted (**the first of these was
subsequently done — see Run 2 below**):

- **Spent-state testing** needs a second transaction that consumes this note (`NoteUsed` +
  a fresh `EncNoteCreated`). Budget: another ~3 STRK of gas + 2 STRK pool fee from the
  public balance — the main account holds 6.391084 STRK, enough for exactly one more pool
  transaction, and `strk20test`'s faucet cooldown lifts ~2026-09-01T14:00Z. A private
  transfer to a *second* registered key would additionally need that key registered
  (another register tx). A self-transfer or a partial withdraw is the cheaper spent-state
  probe.
- **Reorg / masked-supersede paths** are untouched here.
- The helper accounts `fund1` / `fund2` (in `data/sepolia/accounts.json`) are deployed,
  unregistered, and hold ~0.097 STRK each; they are reusable as second-party recipients
  after a future faucet grant.

Unrelated note for whoever reads this next: the working tree picked up modifications under
`crates/` at 17:15 local during this run that did **not** come from this task — this work
touched only `data/sepolia/` and this file.

---

# Run 2 — spending the note under the upgraded class

Run date: 2026-08-31, ~40 minutes after Run 1. Between the two runs **the pool was upgraded
on chain at block 14,339,893** — 778 blocks after our note — from class
`0x56ab118a8a6e38efc93ad758cefe909fee421fa931ce3cf72df624d345623b2` to
`0x07e2bbd7ccc1e68b2695caef70aeb2a3be6cd017b5d5159278ba08f2d8de33f` (`get_version` went
`3288624` = `"2.0"` → `3288625` = `"2.1"`; both OBSERVED).

The coordinator's ABI diff had already established that every event our decoder consumes is
field-level identical across the two classes, and that only an admin event changed
(`OpenNoteDepositorBlockSet` → `OpenNoteScreeningPolicySet` — the same divergence the RC.6
SDK tag shows in Run 1's version table, which now reads as *RC.6 tracking this upgrade*).
What the ABI could not answer: **did the storage layout move?** Discovery walks storage
slots, so only a note written by the new class settles it.

## Result

One transaction did both jobs. OBSERVED:

| | |
|---|---|
| transaction | `0x3d253f8a5ba1d84878b3d4c328e1c6d6fe5a95f163d654de6c9f8776a16f964` |
| block | **14,340,785**, tx_index 1, block hash `0x06aae57e71116d4d575a089136b540721da06545a6c0353d84757be0c200f01e` |
| timestamp | 2026-08-31T15:07:52Z |
| status | `SUCCEEDED` / `ACCEPTED_ON_L2` |
| **pool class hash at that block** | **`0x07e2bbd7ccc1e68b2695caef70aeb2a3be6cd017b5d5159278ba08f2d8de33f`** (vs `0x56ab118a…` at block 14,339,115 — `starknet_getClassHashAt` at both heights confirms the upgrade sits between them) |
| shape | private **self-transfer** of the whole note (SDK `.with(STRK).transfer({ recipient: self, amount })`) |
| fee paid | 2.719346 STRK |

Exactly two pool events, both under the new class:

```
NoteUsed        0x0247fc60d782e0094e7f98c47f277d92a3345d07a436f1f56b27a9b62be2322e
  keys: [selector, 0x06f3769425be9f731773213fb6917264bfda572b2eeda180513d5cf5cbb71662]  # nullifier
  data: []
EncNoteCreated  0x023c20207be8b1ef4430c25eef8ce779c9745ebe04139555ae81bd4f8fdd6ec5
  keys: [selector, 0x03aa1d44c8920593d29297e509a26445e2bc2a6389fa5e8d59fc2e5944553ecd]  # new note id
  data: [0x00a96cff4545e1fd5b6771fc9548f3769994132036e0a644c941a140c49a5245]
```

**The nullifier in the event is `0x06f3769425be9f731773213fb6917264bfda572b2eeda180513d5cf5cbb71662`
— bit-for-bit the value our client predicted for note `0x00ce526b…67ff`.** The nullifier
formula is independently confirmed against the chain.

The new note: id `0x03aa1d44c8920593d29297e509a26445e2bc2a6389fa5e8d59fc2e5944553ecd`,
**3.000000 STRK**, created at block 14,340,785, `open = false`.

Verified state afterwards, all OBSERVED (`verify-spend.mjs`):

- `nullifier_exists(0x06f37694…1662)` → **true**
- `get_note(old note)` still returns its packed value (notes are write-once; spentness lives
  in `nullifiers`, not by clearing the note) — worth knowing for the indexer: **a spent note
  is not erased**, so spent-state must be derived from `NoteUsed` / the nullifier map
- `discoverNotes()` with the *same saved viewing key* now returns **exactly one** unspent
  STRK note — the new one — and the old note is gone from the set

Value flow (`decode-tx.mjs`): 2.000000 STRK from our account → fee collector `0x03e6c6f41…`
(`collect_fee`, confirming Run 1's correction C3 also holds for non-deposit actions), then
`NoteUsed`, then `EncNoteCreated`, then 2.719346 STRK gas. **No `Deposit`, no `Withdrawal`** —
the shielded value never left the pool.

## The storage-layout answer

`storage-check.mjs` recomputes each slot from the Cairo storage declaration
(`notes: Map<felt252, Note>`, `nullifiers: Map<felt252, bool>` at
`packages/privacy/src/privacy.cairo:98,100`) with the ordinary Starknet map formula
`pedersen(sn_keccak(var_name), key) mod (2²⁵¹−256)`, then reads it with
`starknet_getStorageAt` at the block before and the block of each write. OBSERVED:

| key | slot | @creation−1 | @creation | written by class |
|---|---|---|---|---|
| `notes[0x00ce526b…67ff]` | `0x02dbfb901e6081d2d1346e39f64247a66c4678e59abae160937cdb586f0db943` | `0x0` | `0x00cb93562e444a2f62b371b88d6be6350d7a019df3f3455fe4325ed78c514bb4` | `0x56ab118a…` (old) |
| `notes[0x03aa1d44…3ecd]` | `0x02167d40d45099bcf87312a40bb87e26e0fda1e9b2d49a21be7b6a1b89e1ade8` | `0x0` | `0x00a96cff4545e1fd5b6771fc9548f3769994132036e0a644c941a140c49a5245` | **`0x7e2bbd7c…` (new)** |
| `nullifiers[0x06f37694…1662]` | `0x05697b26bb433d4cc303e95e56272c40de9b576df6ad59568696d5ddd1e01d13` | `0x0` | `0x1` | **`0x7e2bbd7c…` (new)** |

Every value read from the slot equals what the contract's own `get_note` /
`nullifier_exists` view returns, and each slot flips from `0x0` to its value at exactly the
creation block.

**Conclusion (OBSERVED, not inferred): the note and nullifier storage layout is unchanged
across the upgrade.** The same slot derivation locates a note written by the old class and a
note written by the new one. A storage-walking indexer needs no change for class
`0x7e2bbd7c…`. Note the scope: this proves it for `notes` and `nullifiers`, which is what
discovery walks; `recipient_channels` / `subchannel_tokens` were not re-derived by hand, but
`discoverNotes` resolving the new note through its channel exercises them end to end and
succeeded.

Aside: the `Note` struct's second slot (`slot + 1`, the `token` field) reads `0x0` for both
notes, matching `get_note`'s `token=0x0` — the token is carried inside `packed_value`, not in
a separate slot. Same under both classes.

## Budget — it fit, with 0.29 STRK to spare

The account held **6.391084 STRK** and the pool fee still had to come out of the *public*
balance, so the allowance had to be re-established (Run 1 consumed all 5 STRK of it:
3 deposit + 2 fee). Two adjustments made it fit:

- **A transfer has no deposit**, so the approve only needs to cover the 2 STRK fee, not
  `deposit + fee`.
- **The `approve` was batched into the same transaction as `apply_actions`** —
  `account.execute([approveCall, applyActionsCall], { tip: 0n, proof, proofFacts })`. This
  works: the proof facts bind the pool's own action span, not the transaction's call list, so
  a preceding ERC-20 call is invisible to the pool's proof check. It saved the ~0.055 STRK of
  a separate approve tx and, more importantly, meant one estimate instead of two.

OBSERVED numbers:

| | |
|---|---|
| estimate | overall **6.102289** STRK, bounds ceiling **6.102289** STRK |
| balance at validation | 6.391084 STRK — clears the ceiling by **0.288795 STRK** |
| actual gas | **2.719346** STRK (again ~2.2× lower than the estimate, exactly as in Run 1) |
| pool fee | 2.000000 STRK |
| balance after | **1.671738** STRK |

The script aborts with an explicit `SHORTFALL: balance X < required resource-bounds ceiling Y`
before submitting if the ceiling ever exceeds the balance — a `DRY_RUN=1` pass was run first
and cost nothing. No faucet accounts were created.

## New facts, beyond Run 1's corrections

**C12 — `collect_fee` charges the 2 STRK on *every* `apply_actions`, not just deposits.** Run
1 established the fee comes from the caller's public balance; this run shows it applies to a
pure note-to-note transfer with no `Deposit` action at all. Any pool interaction costs
2 STRK public + gas. Budget accordingly: **~9 STRK of free public STRK per pool transaction**
is a safe planning figure on Sepolia today (ceiling ~6.1–6.8 observed, fee 2, actual gas
~2.7–3.0).

**C13 — no screening attestation is minted for non-deposit actions, and none is needed.**
`additional_data.signature` was **absent** on this prove response (present in Run 1), the SDK
packed `Option::None`, and the contract accepted it. Consistent with the Cairo: screening is
asserted only for `TransferFrom`-carrying action spans. Practical consequence: **transfer,
withdraw and register do not depend on StarkWare's elliptic-proxy credentials** — only shield
does. That is exactly the split `sepolia-write-path.md` Q3 predicted for a self-hosted
prover, now confirmed from the hosted one's behaviour.

**C14 — self-transfer is supported and warning-free.** `.transfer({ recipient: <own
address>, amount })` built, proved and executed with `warnings: []` — no `USER_LINKAGE`
warning, no contract rejection. It is the cheapest way to produce a `NoteUsed` +
`EncNoteCreated` pair, so it is the right shape for spent-state fixtures.

**C15 — a spent note's storage slot is not cleared.** `get_note(old note)` still returns its
packed value after the spend. Spentness is *only* the `nullifiers` map / the `NoteUsed`
event. An indexer that infers "unspent" from "slot is populated" would be wrong.

**C16 — proving stayed fast and small post-upgrade**: 3.5 s, 227,886-byte proof, 9 proof
facts, 15-felt `apply_actions` calldata (vs 59 for register+deposit). Proving block `head−9`
(correction C7) applied unchanged.

## Artifacts

`data/sepolia/shield/` (gitignored) gained `spend.mjs` (the run, `SHAPE=transfer|withdraw`,
`DRY_RUN=1` supported), `verify-spend.mjs`, `storage-check.mjs`,
`last-spend-callandproof.json`, `dry1.log`, `spend1.log`. The key file
`data/sepolia/viewing-key-strk20test.json` (mode 0600) gained a `transactions.spend_transfer`
entry with the hash, block, block hash, tx_index, fee, spent note id, **nullifier**, new note
id, new note enc data, proving block and `pool_class_hash_at_block`.

## State after Run 2

- Unspent shielded position: **one 3.000000 STRK STRK-note**,
  `0x03aa1d44c8920593d29297e509a26445e2bc2a6389fa5e8d59fc2e5944553ecd`, created at block
  14,340,785 under class `0x7e2bbd7c…`, discoverable with the saved viewing key.
- Public balance **1.671738 STRK** — **not enough for another pool transaction** (needs
  ~6.1 STRK of ceiling). `strk20test`'s faucet cooldown lifts ~2026-09-01T14:00Z; `fund1` /
  `fund2` are deployed with ~0.097 STRK each and are on cooldown until roughly the same time.
- The indexer now has fixtures for **both** classes in a 1,670-block window: a note creation
  at 14,339,115 (old class), a class upgrade at 14,339,893, and a spend + note creation at
  14,340,785 (new class).

---

# Addendum — the scripts now ship in `examples/sepolia/`

Added 2026-08-31, after Run 2. The working copies described above live under
gitignored `data/sepolia/shield/` and are staying there; what ships is a
key-free port of the same code at
[`examples/sepolia/`](../../../examples/sepolia), so the two runs are
reproducible by someone with their own testnet account.

**Only code was copied. No key material was.** The changes that made that true:

- `lib.mjs` no longer hard-codes `DATA_DIR`. The account comes from
  `STRK20_ACCOUNTS_FILE` (an sncast accounts file the operator supplies) or from
  `STRK20_ACCOUNT_PRIVATE_KEY` / `STRK20_ACCOUNT_ADDRESS` in the environment.
- The derived viewing key is written to `STRK20_KEY_FILE`, mode 0600, at a path
  the operator names and which must be outside the repo. `keyFilePath()` throws
  with an explanatory message when the variable is unset, so there is no
  in-repo default to fall into.
- The shipped `save-keys.mjs` **drops `account_private_key` from the key file**.
  The working copy carried it for convenience; the published flow reads the
  account key from the operator's own source every time, so the derived-key file
  holds one secret instead of two.
- `verify-spend.mjs` and `storage-check.mjs` took their note ids, nullifier and
  block numbers from hard-coded constants of Run 1/Run 2. They now read them
  from the operator's key-file transaction record, with env overrides.
- `.env.example` carries placeholders only; `.env` is gitignored at the repo
  root, and `examples/sepolia/.gitignore` additionally excludes `node_modules/`,
  `upstream/`, `.local/`, `accounts.json` and `viewing-key-*.json`.
- `faucet/flow4.py` ships as `faucet/faucet.py` with the two findings that made
  it work written into its docstring: the undocumented required
  `network: "sepolia"` field, and connection pinning for the intermittent
  `POW_CHALLENGE_INVALID`.

**Secret scan of everything created under `examples/`** (run before finishing):

- Every secret-derived string from `data/sepolia/accounts.json`,
  `viewing-key-strk20test.json`, `vk.txt` and `vk_b.txt` — account private keys,
  public keys, salts, the viewing key in hex and decimal — expanded into 79
  variants (raw, lower, upper, 0x-stripped, zero-padded to 64, decimal) and
  searched across every file under `examples/`: **0 hits**.
- Every `0x`-hex literal of 32 or more hex digits under `examples/`: **9 total,
  all public constants** — the Sepolia pool address, the STRK token address, and
  four event selectors (`EncNoteCreated`, `ViewingKeySet`, `Deposit`,
  `NoteUsed`), which are `starknet_keccak` of the event names.
- `private_key` / `viewing_key` / `privateKey` / `secret` / `mnemonic`: hits are
  all env-var names, function parameters, field names and prose. No values.
- No occurrence of the run's account address, note ids, or nullifier, and no
  reference to `data/sepolia` or any absolute local path.

`examples/sepolia/README.md` carries the three corrections that cost the most in
this run — the faucet's `network` field, the head−9 proving block, and the ~9
STRK per pool transaction budget — so the next operator does not rediscover them.
