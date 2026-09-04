# The discovery-core fork

We build against a one-commit fork of `starkware-libs/starknet-privacy`. This
note says what the commit does, why the fork exists, what it is *not* claimed
to do, and how to remove it.

## What is pinned

| | |
|---|---|
| upstream rev | `74841caf0466d122117945e28ed983e2864c8fc1` (tag `CONTRACT_V2_DEPLOYED_MAINNET_2026-07-08`) |
| fork | `https://github.com/kfastov/starknet-privacy.git` |
| branch | `strk20/providers-gate-74841ca` |
| fork head | `e7f0177aab66a9c79699c358b728f9b0324f437e` |
| delta | 1 commit, 1 file, +5/-1 — `crates/discovery-core/Cargo.toml` |
| delta under `discovery-core/src` | **0 lines** |

The commit makes `starknet-providers` an optional dependency behind a feature
that is on by default:

```toml
[features]
default = ["providers"]
providers = ["dep:starknet-providers"]
```

The upstream tag is mutable, so `Cargo.lock` and the `[patch]` in the root
`Cargo.toml` both pin by 40-character rev, never by tag.

## Why a fork and not a source patch

The README and the spec claim the discovery engine is **consumed unmodified**.
That claim is worth keeping literally true, so the fork is allowed to change
packaging (`Cargo.toml`) and nothing else. CI job `fork-delta-check` is the
mechanical form of the claim: it fails if
`git diff upstream..fork -- crates/discovery-core/src` is non-empty, and its
failure message says outright that the "unmodified" sentence has stopped being
true. It also asserts that the `[patch]` rev equals the fork branch head, so the
diff it just took is the code the build compiles. Two further steps used to sit
there — a whole-file-list assertion and a replay of
`patches/discovery-core-providers-gate.patch` against the fork tree — and #17
removed both: neither ever went red, and `Cargo.lock` pins the fork rev anyway.
The consequence is worth stating plainly: **the checked-in patch file is now
verified only for carrying exactly one commit** (`scripts/check-invariants.py`,
check 5). It is documentation of the delta, not evidence for it. Re-export it
with `git format-patch --stdout <upstream>..<fork-ref>` whenever the fork moves;
nothing will tell you if you forget.

`starknet-providers` really is unused at this rev — `grep -rn 'starknet.providers'`
over `crates/discovery-core/src` and `crates/discovery-core/tests` returns
nothing — which is what makes the gate safe rather than clever.

## What the fork buys, measured

With `default-features = false`, the `wasm32-unknown-unknown` dependency graph
of `discovery-core` drops from **142 to 118 crates**. The 24 removed:

```
auto_impl byteorder bytes crunchy ethbloom ethereum-types fixed-hash http
impl-rlp impl-serde log primitive-types reqwest rlp rustc-hex
starknet-rust-providers static_assertions sync_wrapper thiserror
thiserror-impl tiny-keccak uint wasm-bindgen-futures web-sys
```

**What it does not buy — measured 2026-08-31, correcting an earlier
assumption.** The plan that motivated this work assumed `discovery-core` could
not be built for `wasm32-unknown-unknown` because `starknet-providers` drags in
a native HTTP/TLS stack. That is false. `reqwest` has a wasm backend
(`web-sys` + fetch), and

```
cargo build -p discovery-core --target wasm32-unknown-unknown
```

succeeds at the pinned rev with default features and no fork at all. So the
fork is a **dependency-surface reduction, not a target unblocker**, and no
document, commit message or PR body may say otherwise. The things that actually
block our own wasm build are ours, not upstream's: `rusqlite` plus
`tokio::spawn_blocking` in `ClientView`, and `zstd-sys`.

## The patch is workspace-wide, and must stay that way

```toml
[patch."https://github.com/starkware-libs/starknet-privacy.git"]
discovery-core = { git = "https://github.com/kfastov/starknet-privacy.git", rev = "e7f0177…" }
```

One entry, applied to the whole workspace. **A split pin — some crates on the
fork, some on upstream — is forbidden.** It would put two `discovery-core`
builds in one dependency graph and therefore two incompatible `Felt`
identities, which is the same class of breakage documented in
`docs/spec/architecture.md` §3 for `starknet-types-core` 0.2 vs 1.0. Wasm
consumers get the reduced graph via `default-features = false` on their own
dependency edge, not via a second pin.

## Why this is a no-op for the native build

Because the feature is default-on and no source line changed, the resolved
graph is identical apart from where `discovery-core` is fetched from.
Demonstrated at the time of the change:

- `cargo metadata` over all **363** packages: exactly **one** line differs, the
  `discovery-core` source URL. No package added, removed, or re-versioned.
- `Cargo.lock`: exactly **one** line differs, the same source URL.
- `cargo tree -p discovery-core`: **473** lines, identical except the root line.
  `starknet-rust-providers` is still in the graph, confirming default-on.
- `cargo test -p strk20-client -p strk20-feed`: 47 tests, result lines
  byte-identical before and after the patch.

Reproduce the graph check with:

```sh
cargo metadata --format-version 1 | jq -r '.packages[] | "\(.name)|\(.version)|\(.source // "local")"' | sort
```

## The upstream PR

**Status as of 2026-09-02: OPEN, not merged.**
[starkware-libs/starknet-privacy#984](https://github.com/starkware-libs/starknet-privacy/pull/984),
"discovery-core: make unused starknet-providers optional behind a default-on feature".
The `[patch]` in the root `Cargo.toml` stays until it merges.

The rest of this section is the record of how it was drafted, kept because the
reasoning about what the PR may and may not claim still binds anything written
about the fork.

**Before it was opened: not opened, needing a human decision first, for two reasons.**

*It needed your explicit go-ahead.* Opening a PR posts publicly on a third
party's tracker under your name. That is not something an automated track
should do on its own say-so.

*The justification in the plan does not survive measurement.* Plan item 11 says
the PR should argue that the gate "unblocks wasm32 builds of discovery-core".
It does not — see the measured section above; the ungated crate builds for
wasm32 today. What is left to argue is narrower: an unused dependency becomes
optional, and wasm consumers who opt out shed 24 crates. That may or may not be
worth a maintainer's time — your call, not the track's.

Draft body, with the false claim removed:

> `starknet-providers` is declared as a dependency of `discovery-core` but is
> not referenced anywhere under `crates/discovery-core/src` or its tests at this
> revision.
>
> This makes it `optional = true` behind a `providers` feature that is on by
> default, so every existing consumer resolves an identical dependency graph and
> no public API changes. Verified on a downstream workspace: across 363 resolved
> packages, the only difference is where `discovery-core` is fetched from.
>
> The benefit is for consumers building `discovery-core` for a constrained
> target. With `default-features = false` the `wasm32-unknown-unknown` graph
> drops from 142 to 118 crates, removing `starknet-providers`, `reqwest`,
> `web-sys`, `wasm-bindgen-futures` and the `ethereum-types` stack.
>
> To be precise about scope: `discovery-core` already builds for
> `wasm32-unknown-unknown` with default features. This is a reduction in
> dependency surface, not a fix for a broken target.

To submit once you have decided:

```sh
gh pr create --repo starkware-libs/starknet-privacy \
  --base main --head kfastov:strk20/providers-gate-74841ca \
  --title "discovery-core: make unused starknet-providers optional behind a default-on feature" \
  --body-file <the draft above>
```

Note the branch forks from the pinned tag, not from current `main`, so the PR
may need a rebase onto `main` before upstream will take it.

## Removing the fork

If the upstream PR merges, delete in one commit: the `[patch]` section in the
root `Cargo.toml`, `patches/discovery-core-providers-gate.patch`, the
`.github/workflows/fork-delta-check.yml` job, and this file — then repoint
`workspace.dependencies.discovery-core` at the upstream rev that carries the
gate.

If upstream declines, nothing breaks; the fork keeps working and this note
keeps explaining why it exists.

## Re-exporting the patch file

```sh
git -C <clone> format-patch --stdout \
  74841caf0466d122117945e28ed983e2864c8fc1..strk20/providers-gate-74841ca \
  > patches/discovery-core-providers-gate.patch
```
