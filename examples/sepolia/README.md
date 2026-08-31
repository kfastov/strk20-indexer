# Sepolia demo runs — mint a note, then spend it

These are the scripts that produced the two Sepolia transactions the indexer is
tested against: a **shield** (register a viewing key + deposit 3 STRK + create a
note, all in one `apply_actions`) and a **private self-transfer** that spends
that note and creates a new one. They exist so the end-to-end claim is
reproducible by someone who is not us: with your own testnet account you can
create a note, then point `strk20-sync` at a Sepolia feed and watch it find that
note **without ever sending your viewing key anywhere**.

Recorded results of the original runs, with every observed number:
[`docs/research/live/sepolia-shield-run.md`](../../docs/research/live/sepolia-shield-run.md).

> **Testnet only.** Every script that can submit a transaction calls
> `starknet_chainId` first and aborts unless it is `SN_SEPOLIA`. Do not point any
> of this at a mainnet account.

## Secrets

No key material lives in this directory, and none is ever written into the
repository.

- The account private key comes from a path or an environment variable **you**
  supply: `STRK20_ACCOUNTS_FILE` (an `sncast` accounts file) or
  `STRK20_ACCOUNT_PRIVATE_KEY`.
- The **viewing key is derived, never entered**:
  `starknetKeccak("<chainId>:<pool>")` → Stark ECDSA sign with the account key
  (RFC-6979, deterministic) → `poseidon([r,s])` mod curve order → canonicalised
  into the lower half. Identical to upstream `demo/src/session.ts`, so anyone
  holding the account key recomputes the same viewing key. It is printed masked,
  never in full.
- `save-keys.mjs` persists the derived viewing key and the transaction record at
  `STRK20_KEY_FILE`, mode `0600`. Put that path **outside this repository**.
  Unlike the original working copy, the file no longer duplicates the account
  private key.

Start from [`.env.example`](.env.example) — it carries placeholders only.

## Setup

The SDK is not installable from the public npm registry (`npm view
@starkware-libs/starknet-privacy-sdk` returns `403`; the GitHub Packages route
needs a `read:packages` token). It does not matter: `starkware-libs/starknet-privacy`
is a public Apache-2.0 repo and `sdk/` builds standalone with no registry auth.

```sh
git clone https://github.com/starkware-libs/starknet-privacy.git upstream
git -C upstream checkout PRIVACY-0.14.3-RC.5
(cd upstream/sdk && npm ci && npm run build)     # tsc -> dist/
npm install                                       # file:./upstream/sdk + starknet@10.5.0
cp .env.example .env && $EDITOR .env
set -a; . ./.env; set +a
```

Node 26.6.0 runs this headless — no browser, no OHTTP.

**Pin the SDK tag by ABI-matching, not by guessing.** The deployed class's ABI
was diffed against `sdk/src/internal/abi.ts` at each candidate tag: RC.3, RC.4
and RC.5 matched the deployed class on all 117 entries; RC.2 was missing the
`ComputeAndInvoke` variant in both action enums (a silent serialization
mismatch, not a clean error) and RC.6 diverges the other way. Redo that check
after any pool upgrade before trusting a tag.

## The flow

```sh
node status.mjs        # read-only: chain, pool version/fee/screener, balances, prover+discovery health
python3 faucet/faucet.py 0x<address>   # 5 STRK per address, ~24 h cooldown
node save-keys.mjs     # derive + persist the viewing key BEFORE any transaction
node shield.mjs        # approve -> prove -> estimate -> apply_actions   (register + deposit + note)
node verify.mjs        # receipt, the 3 expected events, get_public_key, discoverNotes
node spend.mjs         # private self-transfer: NoteUsed + a fresh EncNoteCreated
node verify-spend.mjs  # nullifier vs the client's prediction, new note, class hash at that block
node storage-check.mjs # do the new class's notes land in the same storage slots? (read-only)
```

`DRY_RUN=1` runs everything up to and including fee estimation and submits
nothing. Use it first — it costs no STRK and catches a shortfall before the
prover is asked for anything.

`shield.mjs` skips `approve` when the allowance already covers `deposit + fee`,
and aborts before submitting if fee estimation fails.

## Three things that will otherwise cost you a run

### 1. The faucet requires an undocumented `network` field

`POST /api/public-agent/faucet/request` needs
`{userAddress, challengeId, nonce, network: "sepolia"}`. Omit `network` and the
claim is rejected even though the proof of work was solved for that exact
address.

That is not the whole story. `POW_CHALLENGE_INVALID` is **intermittent**: with a
fresh address and a freshly solved, verified 20-bit PoW, five separate
submissions were rejected and one succeeded under identical bodies (address
padding, string-vs-number nonce and a settling delay were each ruled out as the
discriminator). Reusing **one keep-alive TCP connection** across
`/pow/challenge` and `/faucet/request` succeeded first try — the API appears to
be load-balanced across instances that do not share challenge storage, so
pinning the connection pins the instance. `faucet/faucet.py` does exactly that.

The grant is **5 STRK per address** (not the 100 the web UI's quota copy
implies), with a per-address cooldown of about 24 hours (`429
ADDRESS_COOLDOWN`). Two addresses were needed to fund one pool transaction.

### 2. The proving block must be head−9

Proving at `latest` — the SDK's own default `blockIdentifier` — fails at fee
estimation:

```
41: Transaction execution error: Invalid proof facts: The proof block number
    14339075 is too recent. The maximum allowed block number is 14339068.
```

i.e. head−7 at that moment. Upstream uses `PROVING_BLOCK_DEPTH = 9`
(`demo/src/hooks/useTransactions.ts`), which also leaves room for the ~4 s
proving round trip. Both scripts prove at `head - 9`
(`STRK20_PROVING_BLOCK_DEPTH`). The Cairo side confirms both ends of the window:
`base_block_number < current_block_number` and
`current_block_number <= base_block_number + proof_validity_blocks`, with
`get_proof_validity_blocks() = 450` on chain. Do not lower the depth; there is
no timeout pressure, since proving took 3.5–3.7 s in both runs.

### 3. Budget ~9 STRK of free public STRK per pool transaction

The 2 STRK pool fee is charged by `collect_fee` from the **caller's public
balance**, before the actions are applied — on *every* `apply_actions`, not just
deposits. Two consequences:

- the ERC-20 `approve` must cover **`deposit + fee`**, not just the deposit
  (approving only the deposit reverts in `collect_fee`);
- a pure note-to-note transfer still costs 2 STRK, so the allowance for the
  spend run only has to cover the fee.

Observed on the two real runs:

| | shield (register + deposit) | private transfer |
|---|---|---|
| estimate / resource-bounds ceiling | 6.796413 STRK | 6.102289 STRK |
| actual gas paid | 3.026531 STRK | 2.719346 STRK |
| pool fee | 2.000000 STRK | 2.000000 STRK |
| deposit | 3.000000 STRK | — |

The **ceiling**, not `overall_fee`, is the number the balance must clear at
validation time; the estimate ran about 2.2× the eventual bill in both runs.
`~9 STRK free public STRK per pool transaction` (ceiling ~6.1–6.8, fee 2, gas
~2.7–3.0) is the safe planning figure — not the ~24 STRK some write-ups suggest.
Both scripts abort with an explicit `SHORTFALL: balance X < required
resource-bounds ceiling Y` before submitting.

Two cost savers the spend run uses: a transfer has no deposit, and the `approve`
is batched into the same transaction as `apply_actions` — the proof facts bind
the pool's own action span, not the transaction's call list, so a preceding
ERC-20 call is invisible to the pool's proof check.

## Other things worth knowing

- **Screening applies only to deposits.** The shield run's prove response
  carried `additional_data.signature` (a SNIP-12 depositor attestation minted by
  the hosted prover); the transfer's did not, and the contract accepted
  `Option::None`. So transfer, withdraw and register do not depend on
  StarkWare's screening credentials — only shield does.
- **A spent note's storage slot is not cleared.** `get_note` still returns the
  packed value after the spend; spentness lives only in the `nullifiers` map and
  the `NoteUsed` event. Anything inferring "unspent" from "the slot is
  populated" is wrong.
- **`self-transfer` is supported and warning-free** — the cheapest way to
  produce a `NoteUsed` + `EncNoteCreated` pair, which is the right shape for
  spent-state fixtures.
- **publicnode Sepolia rejects the `pending` block tag** ("unknown block tag");
  `starkli` needs `--block latest`, and `starknet_getClass` takes the block id as
  a bare tag string (`["latest", classHash]`), not the object form.
- The alpha-sepolia prover and discovery endpoints answered us unauthenticated,
  but nobody from StarkWare has sanctioned that use.
