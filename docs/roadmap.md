# Current release and limits

## Submission

- [Live demo](https://strk20.nullref.cc/demo/): wallet creation and backup,
  funding, account deployment, shield, local discovery, private transfer and withdrawal.
- [Demo video](https://youtu.be/ngoYvunrOsI): 2:24, available by link.
- [Submission metadata](../strk20.json): demo and video URLs, plus seven successful
  mainnet transactions touching the STRK20 pool. The hub verifies all seven;
  its latest snapshot has not yet ingested the video commit.
- [Published SDK](https://www.npmjs.com/package/strk20-discovery): the current
  API is documented in the [SDK README](../ts/strk20-discovery/README.md).
  Releases use GitHub Actions trusted publishing.
- The demo log records explicit wallet operations. Background synchronization
  and cache timings are not attached to the active transaction.
- [Verification and measured runs](spec/demo-app.md) document the completed
  Sepolia and mainnet cycles, recovery behavior and performance conditions.

The repository is the submission; no additional submission PR is required.
The [rules](https://github.com/starkience/strk20-hackathon#submitting) set the
cutoff at September 7, 2026, 23:59 UTC.

## Current limits

- Verification establishes complete pool state at an accepted Starknet RPC
  checkpoint. It does not prove every historical transition or Ethereum finality.
- Local cache storage is trusted. Its checksum detects corruption, not malicious
  replacement; authenticated encryption needs an explicit key and threat model.
- Verification and state serialization share one Worker. Changed state is saved
  in the background, but synchronous serialization still occupies that Worker.
- Cold startup and fresh discovery depend on state size and proof/RPC availability.
  Published measurements are observations, not guaranteed latency bounds.
- Feed hosts see IP addresses and timing. The hosted prover receives proving
  inputs. Optional comparison explicitly discloses a viewing key to the official
  discovery service.
- A complete transaction-history API, larger server refactors and independent
  Ethereum-finalized checkpoints are outside this release.
- The packaging-only [upstream PR](https://github.com/starkware-libs/starknet-privacy/pull/984)
  remains open; its merge is not required to use or submit this package.
