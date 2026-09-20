# Hosting the indexer

The [Dockerfile](../../Dockerfile) builds `strk20`; the
[Compose file](../../docker-compose.yml) runs isolated Mainnet and Sepolia
instances. Each has its own volume, port and health check. Configure DNS, TLS and
a reverse proxy for your deployment. This guide describes procedures, not the
current state of a particular host.

## Startup and configuration

From the repository root:

```sh
docker compose up -d --build          # both networks
# Or select one service:
docker compose up -d --build mainnet

docker compose logs -f mainnet
curl -fsS http://127.0.0.1:8080/health | jq
curl -fsS http://127.0.0.1:8081/health | jq
```

Overrides belong in a private `.env` beside `docker-compose.yml` or in the shell.
The current defaults are in the Compose file and
[network profiles](../../crates/indexerd/src/config.rs).

| Compose variable | Purpose |
|---|---|
| `MAINNET_RPC_URL`, `MAINNET_RPC_FALLBACK` | Primary and fallback HTTP RPC. Need historical blocks, events, state updates and pool reads for backfill. At least one proof-capable endpoint is needed for root checks and proof delivery. |
| `MAINNET_RPC_WS_URL` | New-head subscription used to wake ingestion. |
| `MAINNET_FEEDER_URL` | Optional source of coherent live block/receipt/state-update data. |
| `MAINNET_BIND`, `MAINNET_PORT` | Host listener binding; loopback and port 8080 by default. |
| `MAINNET_ALLOW_CLASS` | Additional decoder allowlist flags, for example `--allow-class 0x...`, after checking compatibility with an upgraded pool. |
| `SEPOLIA_*` | Same controls for the other network; default host port 8081. |
| `RUST_LOG` | Rust logging filter. |

HTTP RPC must support the methods and response shapes used by
[`rpc.rs`](../../crates/indexerd/src/rpc.rs), including
`starknet_getStorageProof` for verification. Historical proof availability is
also required for historical checkpoint reads; a recent proof window cannot
serve every bound. Configure browser access/CORS on client RPCs. No fixed proof
retention or availability is assumed for an arbitrary provider.

Without usable proofs, epoch publication can continue unverified and a healthy
HTTP response does not establish correctness. The server's
[MATCH/MISMATCH/UNAVAILABLE behavior](../spec/architecture.md#verification-and-publication)
and the [client contract](../spec/consumer-path.md) explain the boundary.

For native operation, use the pinned Rust toolchain and:

```sh
cargo build --release --locked -p strk20-indexerd
./target/release/strk20 run --network mainnet --db ./strk20.db --feed-dir ./feed
```

The CLI accepts `STRK20_DB`, `STRK20_FEED_DIR`, `STRK20_NETWORK`, `STRK20_RPC_URL`,
`STRK20_RPC_FALLBACK`, `STRK20_RPC_WS_URL` and `STRK20_FEEDER_URL`; consult
`strk20 run --help` for remaining overrides. Without a WebSocket URL, `run` uses
HTTP polling. The container listens on port 8080 internally; the native default
is `127.0.0.1:8080`.

## Volumes and backups

Each named volume mounts at `/data` and contains:

- `/data/strk20.db` and SQLite `-wal`/`-shm` files.
- `/data/feed/`, including manifest, epochs, head, anchors and snapshots.

Back up the whole volume as one unit. Removing epoch files alone does not cause
normal forward cutting to regenerate them. Keep Mainnet and Sepolia volumes
separate. The image runs as UID 10001; restored data must remain writable by that
user. Do not use `docker compose down -v` when preserving instance data.

For a consistent backup:

1. Record the running image identity, source revision and configuration outside
   the repository. Save any reverse-proxy and static-demo configuration separately.
2. Stop the affected service with `docker compose stop mainnet` (and `sepolia`
   if backing up both). Check that no repair command or other writer uses it.
3. Locate the service's actual named volume through `docker inspect` on
   `docker compose ps -aq mainnet`. Archive all its contents to a new backup path,
   preserving ownership and permissions. Include database and feed from the same
   stopped state; copying a live WAL database and changing feed is not a consistent
   rollback point.
4. Verify the archive inventory and checksum, and keep a copy outside the host.
   If backup fails, restart the existing container before attempting an upgrade.
5. Resume with `docker compose start mainnet` and check head progress.

Restore only when data recovery is needed: stop all writers, retain the failing
volume for diagnosis, restore a complete matching archive into an empty replacement
volume, preserve UID/permissions, then attach it with the matching configuration.
Validate the feed and checkpoint before making it public. Do not overlay a backup
onto a partially newer database/feed pair.

## Networking and caching

The single container port serves all enabled routes. Keep it on loopback and
allowlist these at the public TLS proxy:

- `/feed/…`, including `/feed/live` and `/feed/proofs/{block}`.
- `/health`.
- `/v1/stats`.

Keep `/metrics` on the operator network. `--enable-raw` and `--enable-compat` are
disabled in Compose; if enabled for private use, keep `/v1/raw/*`, `/v1/sync/*`
and `/v1/history` off the public proxy. They reveal queried selectors or receive
viewing keys. Public routes have CORS; private modes do not. CORS is not server
authentication or a substitute for network access controls.

When exposing Mainnet beneath `/mainnet/`, strip that prefix at the proxy so
`/mainnet/feed/live` reaches the service's `/feed/live`. Route each network to its
own backend. Forward the origin's cache policy:

| Artifact | Cache behavior |
|---|---|
| Genesis, epochs and snapshots | `public, max-age=31536000, immutable`. |
| Manifest | `public, max-age=30`, SHA-256 ETag. |
| Head and anchor log | `no-cache`, SHA-256 ETag and conditional reads. |
| Live SSE | `no-cache`, streaming; disable proxy buffering and edge caching. |
| Recent proof endpoint | `no-store` on successful proof responses. |

Epoch ETags hash compressed bytes; `x-content-sha256-raw` identifies the canonical
uncompressed payload. The origin serves whole bodies, without Range/206 support.
Compress JSON at the proxy if needed; zstd artifacts are already compressed.
The full interface is in [Architecture](../spec/architecture.md#http-interfaces).

Keep SSE connections open and forward events promptly. A successful connection
alone is insufficient: verify full startup payloads, subsequent deltas and proof
events through the public proxy. Following an operator recut, purge the rewritten
epoch/snapshot URLs and manifest from caches; indexed filenames can contain new
bytes after this explicit history rewrite.

## Health and upgrades

```sh
docker compose run --rm mainnet status
docker compose run --rm mainnet epoch-verify
docker compose run --rm mainnet verify-root
curl -fsS http://127.0.0.1:8080/health | jq
curl -fsS http://127.0.0.1:8080/metrics
curl -N --max-time 20 http://127.0.0.1:8080/feed/live
```

The bounded SSE command ends at its timeout even on a healthy open stream.
`UNHEALTHY`/HTTP 503 means no head is recorded yet (or a request failed); a cold
backfill may take substantial time. `DEGRADED`/HTTP 200 identifies unknown-class
decoding or a root mismatch, so inspect `decode_state`, `verify_root_failed`,
`mismatch_block` and `reason`. The reported `lag_secs` is currently a constant
zero: compare head progress with the configured RPC rather than using it as a
freshness measurement. Container health alone does not verify proofs or finality.

Upgrade after CI passes for the intended revision:

1. Preserve the current image under a rollback tag, record the revision and save
   configuration. Update a clean deployment checkout with `git pull --ff-only`.
2. Build with `docker compose build mainnet` while existing services run. Both
   services use the same image, so build once.
3. Make consistent volume backups as above, then run
   `docker compose up -d --no-build`.
4. Check both local and public health, advancing heads, logs, epoch integrity,
   SSE delivery and an independent consumer checkpoint verification.
5. To roll back code, retag the saved image as `strk20-indexer:latest`, restore
   matching configuration if necessary, and recreate with `--no-build`. Restore
   volumes only if needed and only while stopped. A static-demo rollback is separate.

## Repairing a mirror

Stop the affected server and back up its volume before repair commands. Preserve
its network, DB and feed configuration on every invocation. For Compose Mainnet:

```sh
docker compose stop mainnet
docker compose run --rm mainnet audit-coverage
docker compose run --rm mainnet enumerate-slots --attribute
```

`audit-coverage` compares event counts and can repair named event-bearing blocks
with `--repair`; it cannot find eventless writes. `enumerate-slots` reports missing,
divergent and extra slots at a provable block. `--attribute` identifies candidate
writing blocks for missing slots, with the
[attribution limits](../spec/architecture.md#recovery) described in architecture.
Use an explicit `--block` when a particular provable height is needed.

Re-ingest the diagnosed block list with `rescan --blocks <comma-separated-blocks>`
or an intentionally bounded range with `rescan --from <first> --to <last>`.
Recheck with `verify-root`. If changed blocks lie inside published epochs, run
`recut-epochs --from-block <lowest-changed-block>` and then `epoch-verify` and
`verify-root` again. Recut is resumable but rewrites history; dependent snapshots
are withdrawn. Purge proxy caches and plan a fresh consumer bootstrap when their
already-applied epoch hashes diverge. Do not clear mismatch metadata manually to
make health look good. Resume the service and verify client state before reopening
public traffic.

`mirror-pull <feed-url>` imports verified feed hashes and identity bindings; it is
not independent chain authentication and does not import the source's snapshot
entry. Use an empty destination and still verify state against your chosen RPC.
A copied mirror can carry the same omission as its source.

## Demo and SDK builds

The [source build](../../README.md#build-from-source) builds the pinned official
SDK and actual WASM before the TypeScript SDK/demo. The maintained SDK setup helper
is [`examples/mainnet/setup.sh`](../../examples/mainnet/setup.sh).

The demo reads these build-time variables:
`VITE_MAINNET_RPC_URL`, `VITE_MAINNET_RPC_WS_URL`, `VITE_SEPOLIA_RPC_URL` and
`VITE_SEPOLIA_RPC_WS_URL`. HTTP overrides are used for both header RPC and direct
proof fallback. URLs are embedded in the public bundle; configure provider access
accordingly. Other network settings are in
[`network.ts`](../../ts/demo/src/network.ts). These are independent of the server's
Compose configuration. Set CSP `connect-src` for the configured feed, HTTP/WS RPC,
prover and optional reference service, and allow the module Worker/WASM assets.

Build with `npm --prefix ts run build`. Back up the existing static directory,
publish `ts/demo/dist/` as a unit, then check asset hashes, the page, startup and
subscriptions. Static hosting does not require restarting indexer containers.
Keep the previous asset set for rollback.

SDK publication is separate from deployment. Bump the package version and lockfile,
run the build/tests and isolated `check:release`, and push the change to `main`.
The [publish workflow](../../.github/workflows/publish-sdk.yml) builds and tests
the archive and publishes through npm OIDC when its trusted publisher is configured:

```sh
gh workflow run publish-sdk.yml --ref main
```

Do not republish an existing accepted version. Serving the demo and upgrading the
backend remain explicit deployment operations.
