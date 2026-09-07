# Hosting the indexer

What is here: a container image, a two-network compose file, and the rules for
what may face the internet. TLS certificates and DNS are a deployment
decision and are deliberately not in this repo.

## Current production deployment

Update, 2026-09-07: the demo uses operator-configured QuickNode HTTP and
WebSocket endpoints automatically on both networks. Build with
`VITE_MAINNET_RPC_URL`, `VITE_MAINNET_RPC_WS_URL`, `VITE_SEPOLIA_RPC_URL` and
`VITE_SEPOLIA_RPC_WS_URL`; these URLs are included in the public browser bundle.
Deployment values live outside the repository. Judges do not enter a URL or
configure a connection. There is no RPC settings form in the demo.

The action picker is a split-button menu; current activity is separate from the
scrollable history. Desktop/mobile layouts and keyboard selection were checked
in an isolated browser. Wallets, history and pending transaction hashes survive
refreshing the page; use Resume for an already sent transaction.

A temporary switch of both indexers to the free QuickNode account encountered
HTTP 429 responses. Both server HTTP/WS configurations were restored from
`/root/strk20-deploy-20260907-quicknode/env.before`; containers remain on the
existing backend image. QuickNode quota is reserved for the demo. Subscriptions
to the two supplied accepted Sepolia transactions returned status in about two
seconds. The existing demo waiter, with an initial not-found test gate,
completed in 1.14/2.23 seconds. These are accepted-transaction subscription
checks, not new-transaction inclusion times.

Latest update, 2026-09-07 (19:52 UTC): backend `4cd7f69`, image
`257cbc9815035a5b19936b73e286757b38df5bc627fa438109028fe510c86d2d`.
The backend was activated at 19:40 UTC with consistent volume backups in
`/root/strk20-deploy-20260907-sse-delta/`; rollback image is
`strk20-indexer:rollback-before-sse-delta`. Both networks are healthy and advancing.
The subsequent static deployment uses source `ffb15c1`, SDK 0.1.5,
entry `index-D7mBwpvE.js` and worker `worker-entry-vezKQu9i.js`.
All five hosted files match the isolated SDK-consumer build byte-for-byte.
Static rollback files are in `/root/strk20-deploy-20260907-wallet-actions/`.
The demo now permits repeated deposit, transfer, withdrawal and discovery without
clearing wallet history, while preserving the pending-transaction gate.

SDK 0.1.5 was published through Actions OIDC, shasum
`aa32d4e20d6cfa9c718cdb62805f06fda3e2c720`. Its npm README matches the current
source and no longer includes old-version instructions. CI passed for both
`4cd7f69` and `ffb15c1`, including the SSE recovery tests; the final SDK/demo
suite passed 17/26 tests respectively, plus isolated package verification.

A fresh public-registry Node/WASM consumer verified mainnet block 14520464 in
43.03 seconds and restored block 14520500 from cache in 366 ms with zero network
requests. The live leg verified 14 distinct blocks across a forced SSE disconnect.
Every reconstructed head matched its canonical SHA-256 ETag. Twelve incremental
head frames totalled 9,710 bytes; their corresponding full NDJSON bodies totalled
2,668,711 bytes. Both connection openings sent a full current tail. The forced
disconnect cut the block-14520492 proof; one automatic proof GET recovered it.
There were no manifest/head GETs during the live leg and no SDK errors. These are
Node measurements through a local forwarding proxy, not browser latency claims.

Large snapshot/epoch downloads now use a 30-second idle deadline and a five-minute
total limit: continuing downloads no longer fail at 30 seconds. RPC/metadata
deadlines remain unchanged. SSE sends only new records and header/trailer changes
when the existing record prefix is unchanged; reconnect, rollover or a changed
prefix replaces the tail. Serialization and WASM verification remain in one Worker.

Earlier update, 2026-09-07 (17:25 UTC): backend `735aeb3`, image
`2a9eb9d69f7da2f65354e3637100ce7ed74dbe0b03a18b7ac54f2f4cc9bbe291`,
and demo entry `index-BmQA0GAP.js`, built in an isolated consumer of the SDK
0.1.2 archive. All five public files match the isolated build byte-for-byte.
Both network containers are healthy and advancing. Complete, consistent volume
backups, previous static files, nginx configuration and environment are in
`/root/strk20-deploy-20260907-efficiency/`; the previous image is tagged
`strk20-indexer:rollback-before-efficiency`. nginx already allows `/feed/*`,
including the new recent-block proof route, and needed no changes.

SDK 0.1.2 was published from `735aeb3` through GitHub Actions OIDC with
provenance (npm shasum `cee23f1038bb158e148e61196c5948a0addfe3f6`). Local
workspace tests, strict Clippy, SDK/demo tests and isolated package checks passed.
CI passed on its second attempt: the unchanged snapshot equivalence test first
observed different `l1_accepted` values across sequential fixture reads. That
test was rerun, not changed or removed.

The SDK now reads verified state while synchronization awaits network I/O and
skips unchanged cache saves. Serialization and verification still run synchronously
in one Worker. Mainnet Node/WASM measurements before deployment showed warm
startup at 357–366 ms; one same-block read still waited 180 ms for serialization
of changed state. No overall network latency improvement is claimed. A cold
mainnet attempt hit the existing download deadline; cold startup remains distinct
from a verified-cache restart.

The feed pushes shared public storage proofs over SSE, with `/feed/proofs/{block}`
for bootstrap and missed events. Independent header selection and Rust verification
remain mandatory. The deployed Sepolia SDK verified blocks 14704720–14704722
with zero proof RPC or proof HTTP requests during the subscription and no errors.
The public mainnet stream delivered head/proof events, but two fresh mainnet SDK
runs timed out downloading cold-start data after obtaining the proof and header;
full mainnet SDK acceptance of this deployment is therefore not confirmed.

Earlier update, 2026-09-07: demo entry `index-wK6em-u-.js`, built in an
isolated consumer of the prepared SDK 0.1.1 archive. Mainnet proof RPC is now
`https://api.cartridge.gg/x/starknet/mainnet`: Lava returned HTTP 410 and
`This endpoint has been discontinued.`, reproduced as a discovery error in the
browser. PublicNode remains the independently selected header/live RPC.
All five deployed files match the isolated build's SHA-256 hashes. The previous
static files, server environment and source patch are backed up in
`/root/strk20-deploy-20260907-112423-proof-rpc/`.

The mainnet container was recreated with its existing image and volumes after
changing only `MAINNET_RPC_FALLBACK` in the deployment environment. Sepolia was
not restarted. Fresh full Node/WASM verification passed on both networks;
mainnet browser restore and explicit discovery then passed in 3.48 and 4.14 seconds.
The acceptance edits are committed and pushed in `17e2b68`. SDK 0.1.1 was
published from `8cba0c9` using GitHub Actions OIDC, with npm provenance. An
isolated install from the public registry passed real Node/WASM cold startup,
offline restart, TypeScript checks and a Vite demo build. The initial release
run's publication succeeded, but an immediate `npm view` read hit a stale
registry replica and made its final step fail; the redundant immediate read
was removed. The published package is available and must not be republished.

The npm trusted publisher authorizes `kfastov/strk20-indexer`, workflow
`publish-sdk.yml`, including direct `npm publish`. Future SDK releases run with
`gh workflow run publish-sdk.yml --ref main` after a version bump and push.
No local npm login or long-lived npm token is needed. This publishes the SDK;
static demo and server deployment remain separate operations.

The earlier static deployment `index-CIUMHzjB.js` added a one-shot receipt
catch-up after terminal WebSocket failure. Mainnet withdrawal and subsequent
local discovery completed; see [the evidence](../spec/demo-app.md#funded-mainnet-cycle-completed-2026-09-07).
Earlier deployment records below retain their original dates and versions.

Verified on 2026-09-06: `root@157.173.104.231`, repository
`/opt/strk20-indexer`, Ubuntu 24.04, Docker Compose and nginx. Mainnet listens
on `127.0.0.1:8080`, Sepolia on `127.0.0.1:8081`. nginx serves Sepolia's
allowlisted endpoints at `https://strk20.nullref.cc/` and mainnet's under
`/mainnet/`. `/demo/` aliases `/var/www/strk20-demo/`. DNS is not proxied through
Cloudflare. The nginx site is `/etc/nginx/sites-available/strk20.conf`.

Deployment remains manual:

1. Commit and push, check CI, and require a clean server checkout. Record the
   previous commit and tag the current image before replacing `latest`.
2. Back up the demo and nginx site. Pull with `git pull --ff-only origin main`;
   build one shared image with `docker compose build mainnet` while both old
   containers still serve requests.
3. Stop both containers briefly and archive each entire volume, including the
   database and feed. Resume the existing containers if either backup fails.
   An archive of a live, mutating SQLite database and feed is not a consistent
   rollback point.
4. `docker compose up -d --no-build`. Check both local and public health, head
   progress, logs, metrics and a complete consumer checkpoint verification.
   Verify full SSE payloads through nginx, not just a successful connection.
5. If Rust consumer/WASM code changed, first run `wasm-pack build --release
   --target web --out-dir pkg --out-name strk20_engine` in `crates/wasm`.
   The npm build copies the existing WASM; it does not compile Rust. Build the
   demo locally with `npm --prefix ts run build`, then publish with
   `rsync -a --delete --delay-updates ts/demo/dist/ root@157.173.104.231:/var/www/strk20-demo/`.
   Check the hosted page and its assets. nginx needs no restart for this step.

The 2026-09-06 deployment runs backend `1dbd6be`, image
`fe756d5309d30bff2b3ed64d5bdc0b090789f3b95bbf0a0ec5cfd471643d1b81`.
The demo runs `54993ef`, built in an isolated consumer of
`strk20-discovery@0.1.0`. CI passed Rust, invariants and TypeScript/WASM, including
installation of the SDK tarball and a browser build outside the repository.
Confirmation now uses transaction-status WebSocket events; the page CSP allows
the two configured WSS hosts. Every public asset matched the isolated build
byte-for-byte. Hosted unfunded wallet creation, backup download, address restore
and funding checks passed on both networks, with no page errors or mobile overflow.
This does not replace the pending funded mainnet withdrawal acceptance.

This was a static-only deployment; the backend was unchanged.
`/root/strk20-deploy-20260906-confirmation/demo-before.tar.gz` preserves demo
`486d448`; `SHA256SUMS` and `activated-at` are beside it. The preceding SDK-release
backup remains at `/root/strk20-deploy-20260906-sdk-release/`.

After the preceding backend activation, the actual Node/WASM consumer independently
verified Sepolia `14643219` and mainnet `14455072`, both `rpc-verified` with
`verificationFailed: false`. Both public health endpoints reported OK and
heads advanced. Public HTTPS SSE delivered complete epoch and head payloads.

The current public `.env` overrides are:

```dotenv
MAINNET_RPC_URL=https://starknet-rpc.publicnode.com
MAINNET_RPC_FALLBACK=https://api.cartridge.gg/x/starknet/mainnet
```

Mainnet ordinary live RPC reads use PublicNode. Proof capability selection
prefers endpoints that have served proofs, allowing Cartridge to serve proofs
without becoming the ordinary live RPC. A pruned-history response can also
route that individual historical read to the archive without changing the
active live endpoint. Sepolia retains Cartridge with PublicNode fallback.
CLI archival defaults are unchanged. These are method/capability roles, not
a claim that either endpoint is always fastest or freshest.

Consistent pre-activation volumes, demo, nginx, `.env` and checksums are in
`/root/strk20-deploy-20260906-watchdog-recovery/`; `activated-at` records the
switch. The rollback image is `strk20-indexer:rollback-0c7a33e` and the backed-up
demo is `9e42967`. Earlier complete rollback points remain in
`/root/strk20-deploy-20260906-root-reuse/` (rollback `9e42967`, prior `.env`
absence recorded), `/root/strk20-deploy-20260906-proof-parallel/` (rollback
`7e444be`) and `/root/strk20-deploy-20260906-demand/` (rollback `778849f`).

Roll back by tagging the saved image as `strk20-indexer:latest` and running
`docker compose up -d --no-build`; restore the matching `.env` separately when
required. Restore volumes only if data recovery is needed, with containers
stopped. Restore the demo from its matching archive separately. Backend and
demo source versions may differ because a client-only fix requires no image
rebuild.

The first Sepolia WebSocket connection after this activation again failed to
acknowledge subscription. It now timed out after two seconds and reconnected
successfully, rather than waiting the former sixty seconds. Transport pings
cannot indefinitely hide a missing subscription or a stalled head stream.
The header liveness deadline is ten seconds; reconnection remains a recovery
path, not a block-polling loop. Mainnet still logged intermittent unavailable
proof checks, followed by successful checks on later blocks. Health alone does
not establish proof-provider availability or L1 finality. Client checkpoint mode verifies accepted Starknet
state, not Ethereum finalization.

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
`MAINNET_RPC_URL`, `MAINNET_RPC_FALLBACK`, `MAINNET_RPC_WS_URL`, `MAINNET_PORT`, `MAINNET_BIND`,
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

Production compose configures `STRK20_RPC_WS_URL`: new-head and reorg
notifications from PublicNode wake ingestion immediately; reconnect also
triggers catch-up from the persisted cursor. Notifications are wakeups, not
trusted pool data. HTTP-only CLI deployments without this setting retain the
explicitly logged polling compatibility mode (also used by deterministic RPC
fixtures). Feed publication itself wakes SSE subscribers directly, without a
file-polling task. Run offline repair commands with the server stopped, then
restart it to publish and announce the repaired state.

## RPC endpoints, and what anchoring needs

The two RPC URLs per network are not interchangeable, because they are asked
two different kinds of question. Blocks and events are served by every
endpoint. `starknet_getStorageProof` is a per-endpoint *capability*, and most
public providers do not implement it at all — anchors, `verify-root`, and every
published snapshot depend on that one method.

Two rules follow, both already enforced in code (§12 B1/B4, LIVE-6):

- A proof refusal is **retried on the same endpoint** within its bounded
  budget before trying the remaining proof endpoints. On a
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

For bounded archival retries, occasional successful proofs can be sufficient.
A low hit rate is not sufficient evidence of usable live discovery latency.
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
