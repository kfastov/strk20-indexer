# The discovery-core fork

The workspace uses a packaging patch to make the unused `starknet-providers`
dependency optional behind a default-on `providers` feature. The fork changes
`crates/discovery-core/Cargo.toml`; engine source under `discovery-core/src` is
unchanged. This reduces the dependency surface when default features are disabled.
It is not a fix for an otherwise unbuildable WASM target.

## Active dependency

[Cargo.toml](../../Cargo.toml) declares the upstream dependency, disables its
default features workspace-wide and redirects it to a fixed fork revision through
one `[patch]` entry. [Cargo.lock](../../Cargo.lock) records the resolved source.
There is no separate native/wasm pin. All consumers must share a compatible
engine and Starknet type identity.

The upstream proposal is
[starkware-libs/starknet-privacy#984](https://github.com/starkware-libs/starknet-privacy/pull/984).
It was checked on September 20, 2026 and remains open. The dependency is retained
until a compatible upstream revision includes the needed feature gate and passes
the workspace checks.

## Check the patch

Two checks have different scopes:

- [`scripts/check-invariants.py`](../../scripts/check-invariants.py) checks offline
  that the dependency redirects to a fork with a 40-character revision and the
  checked-in patch contains one commit. It does not verify the patch's contents,
  replay it, or compare its commit ID with the dependency pin.
- [`fork-delta-check.yml`](../../.github/workflows/fork-delta-check.yml) fetches the
  configured upstream revision and fork branch, checks that the branch head equals
  the `[patch]` revision, and requires an empty diff under `discovery-core/src`.
  It does not assert that every other file is unchanged.

The workflow is the source for `UPSTREAM_REV`, `FORK_REPO` and `FORK_REF`. To
reproduce its source comparison in a clone containing both revisions:

```sh
git -C <clone> diff <upstream-revision>..<fork-revision> -- crates/discovery-core/src
```

No output is required for the unmodified-engine claim. Review the complete diff
as well when changing the fork. A source delta requires correcting the fork or
revising the claim, rather than suppressing the check.

The [patch file](../../patches/discovery-core-providers-gate.patch) documents the
delta. Re-export it whenever the fork changes:

```sh
git -C <clone> format-patch --stdout <upstream-revision>..<fork-revision> \
  > patches/discovery-core-providers-gate.patch
```

## Return to upstream

Check the latest official release and source for the gate, then select a compatible
upstream revision that contains it. Merely merging the PR does not make an older
pinned tag include the change. In one dependency update:

1. Update `workspace.dependencies.discovery-core` and remove its fork `[patch]`.
2. Regenerate the lockfile and verify that there is one engine/type source and
   the intended default-feature behavior.
3. Run the Rust build/tests/lint, WASM build/smoke, SDK/demo tests and isolated
   package checks listed in [CI](../../.github/workflows/ci.yml).
4. Remove the obsolete patch and fork workflow, update the offline invariant
   checker and its documentation, and repair links to this guide if retiring it.

If the upstream change is unavailable or incompatible, keep the fixed fork pin.
