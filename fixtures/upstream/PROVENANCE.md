# Upstream fixtures — provenance

Every file in this directory is copied **byte-for-byte** from
[starkware-libs/starknet-privacy](https://github.com/starkware-libs/starknet-privacy)
(Apache-2.0) at the revision below. None of them is edited here; a fixture that
had to be modified would stop being evidence about upstream.

| | |
|---|---|
| repository | `https://github.com/starkware-libs/starknet-privacy` |
| pinned rev | `74841caf0466d122117945e28ed983e2864c8fc1` |
| tag at that rev | `CONTRACT_V2_DEPLOYED_MAINNET_2026-07-08` (the class deployed on mainnet at block 11,632,886) |
| licence | Apache-2.0 |

| file | upstream path | sha256 |
|---|---|---|
| `cairo-reference-data.json` | `crates/discovery-core/tests/fixtures/cairo-reference-data.json` | `9da197c801ab57aa8f46212e87b58cfc78247c9e79485cf0840859cc52310437` |
| `devnet-state.json` | `crates/discovery-core/tests/fixtures/devnet-state.json` | `ea69c284dc369aca3cb8e23e8b339a981221bb5c07e2edca6992b1b9a5626682` |
| `test_fixtures.rs.ref` | `crates/discovery-core/src/test_fixtures.rs` | `8f6aefb015102e5534cd88455b3ec28a27d566e5cd56f0a27e17bc2a40f51925` |

`test_fixtures.rs.ref` carries the `.ref` suffix so `cargo` never compiles it.
It is kept as the reference loader our own fixture code is checked against, not
as a module.

## What each one is for

- **`cairo-reference-data.json`** — cross-language crypto vectors generated from
  the Cairo contracts (`channel_key`, `note_id`, `nullifier`, masking). Upstream
  ships identical Rust and TypeScript copies; this is the Rust one. Consumed by
  `crates/e2e-tests/tests/conformance.rs`.
- **`devnet-state.json`** — a 48-slot pool storage dump plus the two viewing keys
  that go with it. Consumed by `crates/e2e-tests/src/fixture.rs` and
  `crates/wasm/examples/make_fixture.rs`.

## Re-verifying

From a clone of upstream checked out at the pinned rev:

```sh
shasum -a 256 fixtures/upstream/*.json fixtures/upstream/*.ref
cmp fixtures/upstream/devnet-state.json \
    <upstream>/crates/discovery-core/tests/fixtures/devnet-state.json
```

Last checked byte-identical against that rev: 2026-09-02.
