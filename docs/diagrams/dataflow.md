# Dataflow — chain to wallet, and where the viewing key stops

One diagram for the whole read path: how chain data becomes public static files,
how a wallet turns those files into its own notes, and which arrows the viewing
key is allowed on.

Read the two crossed lines first. They are the product.

```mermaid
flowchart TB
  subgraph CHAIN["Starknet"]
    RPC["JSON-RPC endpoint<br/>getEvents · getStateUpdate<br/>getStorageProof"]
  end

  subgraph SERVER["strk20 server — holds no key, knows no user"]
    ING["ingest<br/>events-first scan of pool-active blocks<br/>single-page ranges, no continuation tokens"]
    DB[("SQLite mirror<br/>blocks · storage_log · events · class_history")]
    CUT["cutter<br/>cuts an epoch only once its whole range<br/>is below l1_accepted · recomputes the<br/>pool storage root from the mirror"]
  end

  subgraph FEED["feed/ — public static files, identical for every wallet"]
    GEN["genesis.json<br/>frozen: chain, pool, epoch size"]
    MAN["manifest.json<br/>the poll target"]
    EP["epochs/NNNNNNNN.strk20e.zst<br/>content-addressed · hash-chained via prev<br/>immutable by construction"]
    HEAD["head.ndjson<br/>unfinalized tail · ETag · no-cache"]
    SNAP["snapshots/latest.sqlite.zst<br/>cold-start convenience"]
    ANC["anchors.ndjson<br/>block · block_hash · storage_root · class<br/>append-only audit trail"]
  end

  subgraph NATIVE["strk20-sync — native wallet or backend"]
    NMIR[("folded mirror<br/>SQLite")]
    NENG["unmodified upstream discovery-core<br/>walks pool storage slots"]
    NKEY(["viewing key + address"])
    NOUT["notes · balances · spent-state"]
  end

  subgraph BROWSER["browser wallet"]
    TS["TypeScript wrapper<br/>fetch · IndexedDB · zstd decompress · SSE<br/>every await lives here"]
    IDB[("IndexedDB<br/>epochs and folded state, opaque blobs<br/>only epoch-derived data — a reorg cannot reach it")]
    WASM["WASM module — a pure computer<br/>bytes in, notes out<br/>no network, no storage, no async"]
    BKEY(["viewing key + address"])
    BOUT["notes · balances · spent-state"]
  end

  OWNRPC["the user's own RPC<br/>optional independent check"]

  RPC --> ING --> DB --> CUT
  CUT --> EP
  CUT --> HEAD
  CUT --> MAN
  CUT --> SNAP
  ING -- "root recomputed at a block inside the proof window<br/>and matched against getStorageProof" --> ANC
  GEN -.-> MAN

  MAN -- "GET, no query string, no body" --> NMIR
  EP --> NMIR
  HEAD --> NMIR
  SNAP --> NMIR
  ANC --> NMIR
  NMIR --> NENG --> NOUT
  NKEY -- "used locally" --> NENG

  MAN -- "GET, no query string, no body" --> TS
  EP --> TS
  HEAD --> TS
  SNAP --> TS
  ANC --> TS
  TS -- "raw NDJSON bytes" --> WASM
  TS <--> IDB
  WASM -- "notes" --> TS --> BOUT
  BKEY -- "passed in-process" --> WASM

  NOUT -.-> OWNRPC
  BOUT -.-> OWNRPC
  OWNRPC -.-> RPC

  NKEY x-- "never sent" --x SERVER
  BKEY x-- "never sent" --x SERVER

  classDef chain fill:#eef4ff,stroke:#4477cc
  classDef server fill:#f6f6f6,stroke:#888
  classDef feed fill:#eefaf0,stroke:#3a9a5a
  classDef secret fill:#ffe9e9,stroke:#cc4444,stroke-width:2px
  class RPC chain
  class ING,DB,CUT server
  class GEN,MAN,EP,HEAD,SNAP,ANC feed
  class NKEY,BKEY secret
```

## How to read it

- **Crossed line (`x—x`) = this never happens.** The viewing key and the address
  never reach the server, in any encoding. That is asserted mechanically in CI by
  a byte-scanner over a full proxy capture, and it was re-checked on live Sepolia
  traffic: two wallets, one holding a real key that finds a note and one holding
  an unrelated key that finds nothing, produced **byte-identical request
  streams** — 609 requests, 64,509 bytes each.
- **Solid arrow = bytes actually move.** Everything the wallet fetches is a
  public static file with no query string and no request body: `genesis.json`,
  `manifest.json`, and a sequence of `epochs/…zst`, plus `head.ndjson` and
  `anchors.ndjson`. Because every wallet requests the same files in the same
  order, the request stream carries no information about who is asking.
- **Dashed arrow = optional or metadata.** The user's own RPC is the one path
  that grounds the feed in the chain without trusting us: `strk20-sync verify`
  proves each discovered note and its spent-state against Starknet state roots,
  with the indexer entirely out of the trust path.

## The two halves of the client

The engine is the same in both, and it is upstream's own `discovery-core`,
consumed unmodified.

| | native | browser |
|---|---|---|
| fetching, decompression, storage, every `await` | Rust client | **TypeScript wrapper** |
| folding + discovery | Rust client | **WASM: bytes in, notes out** |
| persistence | SQLite mirror | IndexedDB blobs |
| reorg handling | rewind above the epoch floor | **none needed** — only epoch-derived state is persisted, and epochs are cut below `l1_accepted` |

`zstd` decompression stays in TypeScript (`zstd-sys` does not build for
`wasm32`), so the module receives raw NDJSON and the compressed file's hash is
checked before decompression. Keeping WASM synchronous and storage-free is what
keeps the engine's `Send` bounds satisfiable and the module testable.

## Why the arrows into `feed/` are shaped that way

- An epoch is cut **only when its entire block range is at or below
  `l1_accepted`**, which makes epochs immutable by construction: a reorg can
  never reach one. Only `head.ndjson`, the unfinalized tail, is ever rewritten.
- Epoch payload bytes are a deterministic function of chain data, so every honest
  mirror produces byte-identical epochs, and each one names its predecessor's
  hash. An omitted or altered block is therefore a **visible fork**, not a silent
  difference.
- `anchors.ndjson` is written only when the mirror's recomputed pool storage root
  equals the root in a `starknet_getStorageProof` response for that block. A
  failure to capture an anchor is never a mirror alarm; a mismatch is.

Sources: [`docs/spec/architecture.md`](../spec/architecture.md) §4.2–4.4,
[`docs/spec/consumer-path.md`](../spec/consumer-path.md) §11,
[`docs/research/live/live-run-findings.md`](../research/live/live-run-findings.md)
sessions 5 and 8.
