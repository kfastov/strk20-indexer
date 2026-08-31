# Hosting the indexer

What is here: a container image, a two-network compose file, and the rules for
what may face the internet. TLS certificates and DNS are a deployment
decision and are deliberately not in this repo.

## Run it

```sh
docker compose up -d --build          # both networks
docker compose up -d --build mainnet  # just one

curl -s localhost:8080/health | jq    # mainnet
curl -s localhost:8081/health | jq    # sepolia
```

The image's entrypoint is the `strk20` binary itself, so every subcommand is
available against a service's volume:

```sh
docker compose run --rm mainnet status
docker compose run --rm mainnet epoch-verify
docker compose run --rm mainnet verify-root
docker compose run --rm mainnet mirror-pull https://some-host/feed
```

Overrides go in a `.env` file next to `docker-compose.yml` (it is gitignored):
`MAINNET_RPC_URL`, `MAINNET_RPC_FALLBACK`, `MAINNET_PORT`, `MAINNET_BIND`,
`MAINNET_ALLOW_CLASS`, and the `SEPOLIA_*` equivalents, plus `RUST_LOG`. Every
one of them is a wrapper over a flag the CLI already has; there is no
compose-only configuration.

`MAINNET_ALLOW_CLASS` is the one that is not an environment variable in the
CLI, so compose interpolates it into the argument list instead:
`MAINNET_ALLOW_CLASS="--allow-class 0x..."`. It stays empty in normal
operation — both network profiles already carry every class hash their pool
has ever run — and exists for the recovery path after an upgrade the profile
does not know about yet, where the choice is between adding the class and
letting the decoder go degraded.

## RPC endpoints, and what anchoring needs

The two RPC URLs per network are not interchangeable, because they are asked
two different kinds of question. Blocks and events are served by every
endpoint. `starknet_getStorageProof` is a per-endpoint *capability*, and most
public providers do not implement it at all — anchors, `verify-root`, and every
published snapshot depend on that one method.

Two rules follow, both already enforced in code (§12 B1/B4, LIVE-6):

- A proof refusal is **retried on the same endpoint**, not failed over. On a
  load-balanced pool only some backends carry archive tries, so a single
  refusal means "this backend cannot", not "this block cannot".
- A proof refusal **never moves the active endpoint**. Failing a proof over
  onto a proof-less provider would turn a capability gap into a false mirror
  mismatch.

So a proof-less *fallback* is harmless — it still serves blocks and events.
What is not harmless is having no proof-capable endpoint in the pair at all.

### What an operator without a proof-capable endpoint sees

This is a supported state, not a crash. The indexer stays quiet about what it
could not check rather than pretending it checked:

| symptom | why |
|---|---|
| `feed/anchors.ndjson` stays empty | no proof was ever obtained, so no anchor was captured |
| nothing appears under `feed/snapshots/` | the §11.3 publication gate needs an anchor to ground a snapshot |
| `verify-root` prints `UNAVAILABLE`, exit status 0 | "we could not check" is not "the mirror is wrong" |
| `verify_root_failed` stays false, health stays healthy | a capability gap must never latch a failure (LIVE-4/6) |
| epochs are still cut, served and hash-chained | the feed itself does not depend on proofs |

Such an instance is still useful — it publishes a correct, hash-chained feed —
but its consumers cannot reach the anchored trust grade, because nothing binds
the mirror to a state root the chain signed. If that grade matters to you,
treat a proof-capable endpoint as part of the deployment, not an optimisation.

### Checking an endpoint before you deploy

Never judge a pool on one call: a single refusal from a load-balanced endpoint
tells you nothing. Retry, then confirm the proof is for the block you asked
for.

```sh
RPC=https://rpc.starknet.lava.build
BLOCK=14158970
POOL=0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a

for i in $(seq 1 10); do
  curl -s -X POST "$RPC" -H 'content-type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"starknet_getStorageProof\",
         \"params\":{\"block_id\":{\"block_number\":$BLOCK},
                     \"contract_addresses\":[\"$POOL\"]}}" \
  | grep -q '"result"' && echo hit || echo miss
done
```

A usable endpoint does not need a high hit rate, it needs a non-zero one.
Measured against the mainnet default `rpc.starknet.lava.build` on 2026-09-01,
twelve attempts per block, head at 14,168,818:

| block | behind head | hits |
|---|---|---|
| 14,158,970 (epoch boundary) | ~9.8k | 5 / 12 |
| 11,263,135 | ~2.9M | 3 / 12 |
| 9,000,000 | ~5.2M | 5 / 12 |

Roughly one attempt in two to one in four, at every depth including deep
history, with `global_roots.block_hash` equal to the real header hash every
time. `PROOF_RETRIES` (16 per endpoint) is sized for exactly this, which is why
"the epoch boundary is thousands of blocks behind head" is not an obstacle.

Per-endpoint capability and the measurement method are in
`docs/research/live/proof-window.md`; the per-network defaults, and why each was
chosen, are commented at the constants in `crates/indexerd/src/config.rs`.

## Volumes

One volume per network, mounted at `/data`, holding both halves of an
instance's state:

| path | what it is |
|---|---|
| `/data/strk20.db` (+ `-wal`, `-shm`) | the working index: blocks, events, storage log, epoch rows |
| `/data/feed/` | the published product: `manifest.json`, `head.ndjson`, `anchors.ndjson`, `genesis.json`, `epochs/`, `snapshots/` |

They live together because an epoch file only means anything next to the
database rows whose hash chain names it. The database is the source of truth
and the feed is generated from it — but epoch cutting only ever moves
*forward* from `last_epoch`, so deleting published epoch files does not cause
them to be regenerated. Back up the volume as a unit.

Mainnet and Sepolia never share a volume. They are different chains with
different pools, and an instance that mixed them would be publishing a lie;
`mirror-pull` already refuses a manifest whose `chain_id` disagrees with its
own profile, and this is the same rule one layer out.

## Ports, and what must not face the internet

The container publishes one port, 8080, and the whole router is on it. That
matters: if you turn on the optional modes you cannot separate them by port,
so the restriction has to happen at the reverse proxy.

**Safe to expose.** These take no user-derived parameter — that absence is the
privacy mechanism, not a policy on top of it.

- `/feed/*` — the feed: manifest, head, anchors, epochs, snapshots, and the
  `/feed/live` SSE stream
- `/health`
- `/v1/stats`

**Never expose.**

- `/v1/raw/*` (`--enable-raw`) — a targeted read. What you ask for is what you
  disclose: the server learns which slots and which event keys interest you.
  Responses carry `x-strk20-privacy: targeted-mode-leaks-queried-slots` so the
  mode is never silently in play.
- `/v1/sync/*`, `/v1/history` (`--enable-compat`) — the reference-compatible
  API, which receives **raw viewing keys in request bodies**. Self-hosted, on
  your own box, or not at all.
- `/metrics` — operator data with no authentication.

Neither optional mode is enabled in `docker-compose.yml`. If you enable one,
bind the service to loopback (`MAINNET_BIND=127.0.0.1`, which is already the
default) and put the public listener behind a proxy that allowlists the three
public prefixes rather than denylisting the private ones.

### CORS

The public surface answers with:

```
Access-Control-Allow-Origin: *
Access-Control-Allow-Methods: GET, HEAD, OPTIONS
Access-Control-Expose-Headers: ETag
```

and answers `OPTIONS` preflights with `204` plus
`Access-Control-Allow-Headers: If-None-Match, If-Modified-Since, Accept,
Cache-Control, Last-Event-ID` and `Access-Control-Max-Age: 600`.
`If-None-Match` is named explicitly because it is not a CORS-safelisted
request header, and a browser client that revalidates by hand rather than
leaving it to the HTTP cache would otherwise have its preflight rejected.
`ETag` is exposed because a validator a script cannot read back is a
conditional-GET path that browsers cannot use.

`/v1/raw/*`, `/v1/sync/*` and `/metrics` get **no** CORS headers, and that is
load-bearing rather than an oversight: without them a hostile page cannot make
a visitor's browser query the leaky modes on a host the visitor can reach and
read the answer back out.

`Access-Control-Allow-Origin` is the literal `*` and never a reflected
`Origin`, so no `Vary: Origin` is emitted and one cached object at the edge
serves every origin.

## Caching, ETags, and the CDN

What each feed response carries today:

| path | `Cache-Control` | `ETag` |
|---|---|---|
| `/feed/genesis.json` | `public, max-age=31536000, immutable` | — |
| `/feed/epochs/{n}.strk20e.zst` | `public, max-age=31536000, immutable` | strong, sha256 of the `.zst` bytes |
| `/feed/epochs/{n}.anchor.json` | `public, max-age=31536000, immutable` | — |
| `/feed/snapshots/*` | `public, max-age=31536000, immutable` | — |
| `/feed/manifest.json` | `public, max-age=30` | strong, sha256 of the file |
| `/feed/head.ndjson` | `no-cache` | strong, sha256 of the file |
| `/feed/anchors.ndjson` | `no-cache` | strong, sha256 of the file |
| `/feed/live` | `no-cache`, `x-accel-buffering: no` | n/a (SSE) |

Every ETag is strong — `"<64 hex>"`, no `W/` — because they are hashes of the
exact bytes on the wire, not of a semantic equivalent.

An epoch file carries **two** hashes and they are not the same number. The
`ETag` is the hash of the compressed `.zst` bytes, which is what an HTTP
validator has to identify; `x-content-sha256-raw` is the hash of the
*decompressed* payload, which is the value a mirror checks against the
manifest's hash chain. Both come out of the epochs table, so neither costs a
pass over the file.

The CDN story that follows from the table:

- **Immutable artifacts cache forever.** Epoch and snapshot filenames are
  fixed by epoch index and their contents are pinned by the manifest hash
  chain, so there is no purge story to get wrong. This is where the bytes are
  — 16 MB across 519 epochs on mainnet today against 146 KB of manifest.
- **The manifest absorbs the polling burst.** It is the mutable index every
  client fetches and it grows without bound. `max-age=30` lets the edge
  collapse a burst into one origin fetch; the ETag turns the revalidation
  after those 30 seconds into a bodyless `304` instead of a re-transfer of the
  whole file.
- **The head and the anchor log revalidate every time.** `no-cache` is the
  honest answer for a tail that a grounded client refetches on every sync. The
  ETag still saves the body. If you want the edge to absorb these too, add
  `s-maxage` at the CDN and accept exactly that much staleness on the tail —
  do it at the CDN, not in the origin's headers, so the trade-off is visible
  where it is made.
- **`/feed/live` must bypass the edge.** A buffering proxy holds pokes until
  enough bytes accumulate; `x-accel-buffering: no` handles nginx, other proxies
  need their own streaming setting.
- **Two things the origin does not do.** It serves whole bodies only, with no
  `Range`/`206` support, and it does not compress. Epoch files are already
  zstd so that costs nothing there, but the 146 KB of JSON in `manifest.json`
  is worth gzip or brotli at the proxy or CDN layer.

## Health and cold starts

`HEALTHCHECK` polls `/health`, and `restart: unless-stopped` keeps a crashed
container coming back.

The thing to know before watching a red container: `/health` is `UNHEALTHY`
until the first ingest cycle finishes and writes a head, and on a cold mainnet
start that first cycle *is* the whole backfill — about 70 minutes measured.
That is why `start_period` is 90 minutes. Plain compose does not act on health
(only Swarm restarts on it), so an unhealthy container is a status signal, not
a restart loop.

`DEGRADED` rather than `UNHEALTHY` means the instance is serving but something
is wrong on the verification side — a decoder that met a class it does not
know, or a latched `verify-root` mismatch. `/health` names which.

To skip the cold backfill entirely, seed the volume from a mirror that has
already done it:

```sh
docker compose run --rm mainnet mirror-pull https://some-host/feed
docker compose up -d mainnet
```

`mirror-pull` verifies the hash chain and the chain/pool binding of every
epoch before it stores anything, and it does not copy the source's snapshot
entry — this instance publishes its own after its first cut.

## Image notes

The runtime is `debian-slim`, not distroless or scratch, and that is forced:
`libsqlite3-sys` is built `bundled` and `zstd-sys` compiles vendored C, so the
binary links glibc. `ca-certificates` is required, not decorative — TLS roots
are resolved through the OS trust store, and without the package every RPC
call fails with an unknown-issuer error. `curl` is there for the healthcheck.

The process runs as uid 10001. `/data` is chowned in the image, which is what
makes a fresh named volume writable without an entrypoint chown dance.

`--listen 0.0.0.0:8080` is in the container's arguments, never in the binary's
default. Binding every interface is fine inside a network namespace; the
in-code default stays `127.0.0.1` so that running the binary on a host machine
cannot accidentally publish an unproxied indexer to the LAN.
