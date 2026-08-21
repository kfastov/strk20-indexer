# strk20-indexer

Open, self-hostable note indexer and explorer for the STRK20 privacy pool on Starknet. Written in Rust.

Wallets sync without sending their viewing key anywhere. Apps get a fast, cached discovery API and a public view of what happens inside the pool.

## Why

The STRK20 pool contract emits events only for deposits, withdrawals and key registration. Private transfers and notes leave no events. To find its notes, a wallet has to walk contract storage and decrypt slots. The reference discovery service does that work for the wallet, with two costs:

- The wallet sends its viewing key to the service on every request. The operator sees who syncs, when, how many notes they hold and which tokens they use.
- The service keeps no database. Every sync is a burst of `getStorageAt` calls against an RPC node, roughly two reads per note, about one second per user at low load.

The reference architecture document names a hot storage cache fed by an indexer as the recommended end state and marks both as deferred to a future phase. This project builds that phase and opens it up.

## What it does

**Ingest.** Reads per-block state updates for the pool contract, stores every storage diff, nullifier and public shield/unshield leg in PostgreSQL. Cursor-based resume, reorg rollback, backfill from pool deployment. The index implements the storage backend trait from `discovery-core`, so the reference discovery engine runs on top of it unchanged.

**Discovery API, compatible mode.** Same endpoints as the reference service (`/v1/sync/incoming_state`, `/v1/sync/outgoing_state`, `/v1/sync/preflight_check`), served from the index instead of live RPC. Point the Privacy SDK at this URL and nothing else changes.

**Discovery API, keyless mode.** The wallet fetches its encrypted channel list by public address, decrypts the channel key locally, computes its own subchannel, note and nullifier slots, and asks the indexer for those slots by address. Or it fetches all diffs for a block range and filters locally. Either way no key leaves the device. A WebSocket stream pushes new slots as blocks land, so payment backends stop polling.

**Explorer.** Shield and unshield volume per token over time, registrations, and the part no event indexer can see: note, channel and nullifier counts, growth of the anonymity set per token, and which anonymizer contracts get called. Prometheus metrics and a Grafana dashboard are included.

**CLI.** `strk20 shield`, `strk20 transfer`, `strk20 unshield`, `strk20 sync` for scripts and bots that do not want a Node runtime.

## Quick start

```bash
docker compose up
```

This starts the indexer, PostgreSQL, the API on `:8080` and the explorer on `:3000`. Set `STARKNET_RPC_URL` in `.env` first. Backfill of mainnet takes minutes.

## Using it from a wallet or app

With the official TypeScript Privacy SDK:

```ts
const sdk = new PrivacySDK({ discoveryUrl: "https://your-host:8080" });
```

Keyless sync:

```
GET /v2/slots?from_block=N&to_block=M
GET /v2/nullifiers?from_block=N
WS  /v2/stream?from_block=N
```

Full API reference: `docs/api.md`.

## Status

Built during the STRK20 Private Sprint, August 2026. Mainnet transactions, contracts and demo are listed in `strk20.json`.

- [ ] Ingest and backfill
- [ ] Compatible discovery API
- [ ] Keyless API and stream
- [ ] Explorer
- [ ] CLI

## Honest limits

- In keyless mode the indexer still learns which slots a client asks for, the same thing an RPC node learns today. Range mode hides even that at the cost of bandwidth.
- Storage layout is versioned by block height. A pool upgrade needs a layout update here.
- The compatible mode still receives viewing keys. Use it behind your own deployment, or use keyless mode.

## License

Apache-2.0
