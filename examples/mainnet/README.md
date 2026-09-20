# Mainnet command-line examples

These scripts create a local software wallet and use the official transaction
builder with this project's `NodeDiscoveryProvider` for a Mainnet STRK20 cycle.
For the maintained browser workflow and its recovery UI, see
[Demo workflow](../../docs/spec/demo-app.md).

## Setup

Requires Node 24+, Git, the pinned Rust toolchain, wasm-pack and Brotli. From the
repository root:

```sh
./examples/mainnet/setup.sh
./crates/wasm/build.sh
npm --prefix ts ci
npm --prefix ts run build --workspace strk20-discovery
cd examples/mainnet
cp .env.example .env
$EDITOR .env
set -a; . ./.env; set +a
```

The scripts read environment variables; they do not automatically load `.env`.
[`setup.sh`](setup.sh) builds the pinned official SDK from source into `vendor/`
and installs dependencies without a GitHub Packages token. The tag/repository
settings are owned by that script; `STRK20_SDK_TAG` and `STRK20_SDK_REPO` override
them. An override requires checking compatibility with the configured pool and
rerunning the SDK tests, not just choosing a similar version number.

Configuration is in [`.env.example`](.env.example) and [`lib.mjs`](lib.mjs).
`STRK20_RPC` must report Mainnet and the required RPC version. `STRK20_FEED` must
be an HTTP(S) feed for the transaction scripts; `STRK20_PROOF_RPC` serves proofs.
`STRK20_PROVER` receives proving inputs, including inputs during dry runs.
The pool's fee, class, screening requirement and proof window are read at runtime.

## Wallet and costs

`01-create-account.mjs` creates signing/viewing keys, checks the account address
and prints funding guidance. It does not submit a transaction, but performs public
RPC reads. The viewing key is bound to the chain and pool. Back up the whole
`STRK20_KEYSTORE` directory (default `~/.strk20/mainnet-keystore`) privately; its
`account.json`, `viewing-key.hex` and `state.json` contain recovery material.
Files use mode 0600 inside a mode-0700 directory. Do not commit or print keys.

Fund the displayed address with STRK. Each pool action needs public balance for
the current pool fee and gas; shield also needs the deposit. Private balance alone
cannot pay those public charges. Resource-bound ceilings can exceed actual fees.
Read each live estimate rather than using a historical cost table. The script's
initial funding recommendation is a planning estimate, not a guarantee that every
later step is affordable.

## Run the steps

Explicitly dry-run each transaction first. `DRY_RUN=1` builds, proves and estimates
without submitting; it still contacts the prover. The transaction scripts submit
when dry-run is disabled, including when `DRY_RUN` is unset.

```sh
node 01-create-account.mjs
# Fund its displayed Mainnet address, then deploy:
DRY_RUN=1 node 02-deploy-account.mjs
DRY_RUN=0 node 02-deploy-account.mjs

DRY_RUN=1 node 03-shield.mjs
DRY_RUN=0 node 03-shield.mjs

# Wait for discovered notes to meet the pool's maturity requirements.
DRY_RUN=1 node 04-transfer.mjs
DRY_RUN=0 node 04-transfer.mjs

DRY_RUN=1 node 05-withdraw.mjs
DRY_RUN=0 node 05-withdraw.mjs
```

`03-shield` registers if needed and deposits `STRK20_DEPOSIT_STRK`. Transfer and
withdrawal default to the full spendable amount and the wallet's own address;
`STRK20_TRANSFER_*` and `STRK20_WITHDRAW_*` select an amount/recipient. Another
private recipient must be registered. The local provider's reads are pinned to
one proving block; cold discovery can require extra maturity blocks. Its trusted
private cache is stored beside wallet files.

The optional native discovery helper accepts an HTTP feed or a local feed directory:

```sh
export STRK20_FEED=http://127.0.0.1:8080/feed
./06-discover.sh --json
```

It builds/runs `strk20-sync` with the saved viewing key and network binding.
For server startup, see [Hosting](../../docs/ops/hosting.md); for the native
client's opt-in verification boundary, see
[Consumer path](../../docs/spec/consumer-path.md#native-client-boundary).

## Failures and repeat runs

Completed actions are recorded only after successful receipts, and scripts refuse
to repeat them unless `FORCE=1` is set. That is a completed-step guard, not the
demo's persistent pending-transaction recovery. If a script printed a transaction
hash and then failed waiting for confirmation, check that hash on chain before
rerunning. A missing completed record does not prove nothing was submitted.
`FORCE=1` can spend funds again and can overwrite an existing wallet during creation.

| Failure | Next step |
|---|---|
| Wrong chain or unsupported RPC version | Correct `STRK20_RPC` before proceeding. |
| Insufficient public balance | Read the reported shortfall and estimate; fund gas/fees as well as the deposit. |
| No spendable notes or a checkpoint below snapshot basis | Confirm the earlier transaction, allow maturity/feed catch-up, and retry discovery. |
| Prover unavailable or no required screening attestation | Resolve the prover/screening dependency; no transaction is submitted by that failed build. |
| Transaction reverted | Inspect its receipt; gas may have been charged. Confirm the cause before retrying. |
| Confirmation failure after a hash was printed | Inspect the existing hash first; avoid a duplicate send. |

The endpoints configured in source are dependencies, not availability guarantees.
Local discovery protects viewing keys from the indexer; it does not hide proving
inputs from the hosted prover or network metadata from feed/RPC operators.
