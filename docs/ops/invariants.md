# Invariant checks

`scripts/check-invariants.py` — no arguments, no config, ~1 second, stdlib Python 3.9+.

```
./scripts/check-invariants.py
```

One line per check with `PASS` / `WARN` / `FAIL` and a count, then details only for
what is not clean. **Exit code is 1 if any check FAILs**; a `WARN` never fails the
run — it is a shortlist for a human to eyeball, not a verdict.

It exists so a reviewer stops re-deriving six answers by hand every time. It is a
grep with a memory, not a proof: passing means none of the *known* ways to break
these invariants are present, not that the invariant holds.

Python rather than shell for three concrete reasons: check 2 must ignore matches
inside Rust comments (a line-based `grep -v '//'` cannot tell a doc comment from a
`//` inside a URL literal, and this repo has doc comments that literally say "no
rusqlite here"); check 4 is a set intersection of harvested key material against
every tracked file, not a grep; check 1 compiles an allowlist parsed out of Rust
source into path matchers.

---

## 1. Feed URLs carry nothing user-derived

**Protects** the keyless guarantee: every client's feed requests are byte-identical,
so the server learns nothing about who is asking. The moment one request encodes an
address, a viewing key or an owner, address-blindness is gone — and it goes quietly,
because everything still works.

**How.** The closed allowlist in `crates/e2e-tests/src/feed_urls.rs` (`PATTERNS`) is
the single source of truth; the script parses it rather than restating it. Across
`crates/{client,consumer,wasm,feed}/src` it judges two populations of string
literal: anything shaped like a feed artifact path, plus anything passed to a fetch
primitive (`get_bytes`, `get_optional`, `http.get`) whether artifact-shaped or not —
the second is what catches a brand-new endpoint like `notes/{address}.json` that the
first cannot recognise. A literal fails if it interpolates a user-derived name
(`address`, `owner`, `vk`, `key`, `nullifier`, …), carries a query string, or does
not match an allowed whole path. Literals with no `/` are treated as fragments
(`dir.join(..)`, a `strip_suffix` test) and may only match the tail of an allowed
pattern — they can never introduce a new path component.

**When it fires.** Do not add a pattern to `feed_urls.rs` to make it go away — that
file is the invariant, not a suppression list. Either the request genuinely needs a
new *parameterless* artifact (add it there deliberately, and to the spec), or the
code is about to leak a selector and must be rewritten to fetch a public artifact
and filter locally.

## 2. Block B stays wasm-portable

**Protects** the architectural seam that lets the browser run the same engine as the
native client. `crates/consumer` (Block B) and `crates/wasm` must not reference
`rusqlite`, `tokio`, `reqwest`, or filesystem APIs. Not a style rule: any of them
breaks `wasm32-unknown-unknown`, and the moment the browser needs its own engine we
have two implementations that are supposed to agree.

**How.** Comments, doc comments and string literals are stripped before matching, so
the existing `//! no rusqlite, no tokio, no reqwest` comments do not trip it. Bodies
of `#[cfg(test)] mod` are stripped too — `crates/wasm` legitimately has `tokio` as a
*dev*-dependency for the native fixture generator. The `[dependencies]` table of both
crates' `Cargo.toml` is checked as well, which catches a dep landing before any code
uses it.

**When it fires.** The host capability belongs on the other side of the seam. The
transport trait (`crates/consumer/src/transport.rs`) is the boundary: give the
capability to the host (native transport in `crates/client`, TypeScript in the
browser) and let Block B take bytes. If a dev-dependency moved into
`[dependencies]`, move it back.

## 3. No silent truncation

**Protects** against the worst defect this project has had (LIVE-8): a scan that
dropped data without erroring. Everything downstream — the epoch hash chain, the
snapshot ladder, the report — is consistent with a truncated scan, so nothing
catches it later.

**How.** One hard check and three heuristics over `crates/indexerd/src`:

- **FAIL, and the important one:** the LIVE-8 guard is asserted by *presence*.
  `ingest.rs` must still contain `page.continuation_token.is_some()`. A `getEvents`
  continuation token is node-local state; the scan is correct only because every
  window is answered in a single page and a token in the reply is refused loudly.
  Delete that guard and truncation is silently accepted again.
- **FAIL:** a `.take(` bounding chain data (`events`, `blocks`, `page`, `chunk`, …)
  in `ingest.rs` or `rpc.rs` — a cap on the fetch path is data loss by construction.
- **FAIL:** a discarded `Result` (`let _ =`, `.ok();`) whose expression is a
  persistence call (`commit`, `insert`, `write_all`, `flush`, `execute_batch`, …).
  Losing a write error is silent data loss.
- **WARN:** every other `.take(`, every other discarded `Result`, and any `break`
  with neither an explaining comment nor a visible guard in the preceding lines.

**When it fires.** Expect legitimate WARNs — display paths cap output, idempotent
DDL discards errors on purpose. Read them, satisfy yourself, move on; if a WARN is
permanently fine, the honest fix is a one-line comment saying why, which also
silences the `break` heuristic. A FAIL is not a heuristic: treat it as a data-loss
bug until proven otherwise.

## 4. No secrets in tracked files

**Protects** against committing key material. Real private keys and viewing keys live
only in gitignored places: `data/`, `~/.strk20/`, `examples/*/keystore/`, `.env`.

**How.** Two independent detectors. (a) It reads the actual gitignored key files and
harvests their secret values, then looks for those exact values in every tracked
file — zero false positives by construction. (b) Shape heuristics for when those
files are absent: a secret-*named* assignment holding long hex, a PEM private-key
header, provider tokens.

The important negative: harvesting is restricted to *secret-named fields*
(`private_key`, `viewing_key`, `seed`, …) and explicitly vetoes public-named ones
(`address`, `public_key`, `class_hash`, `token`, `pool`). The pool address, the STRK
token and account class hashes are 64-hex too and appear in tracked source, docs and
fixtures on purpose. An earlier hand-run of this check flagged exactly those; both
the field veto and the "long hex alone is never a finding" rule exist to stop that.
`Cargo.lock` is exempt from the shape pass (its `checksum =` lines are 64-hex).

**When it fires.** Assume the key is burned. Rotate it first, then remove it from the
working tree and from history — the commit is what matters, not the file. Then work
out which gitignore rule should have caught it. Never add an exception to the script
to make a real key pass.

## 5. Upstream consumed unmodified

**Protects** the README and spec claim that `discovery-core` runs unpatched apart
from one dependency-gating commit. The `[patch]` in the root `Cargo.toml` redirects
it to our fork; that is only acceptable while the fork changes packaging and nothing
else.

**How, offline:** the `[patch]` must exist, point at our fork (not `starkware-libs`),
and pin a 40-hex `rev` — a branch or tag pin lets the fork move under us.
`patches/discovery-core-providers-gate.patch` must carry exactly one commit, whose
sha equals the pinned rev, touching only `Cargo.toml` paths and zero lines under
`discovery-core/src`.

This is the local half. CI job `fork-delta-check` does the half that needs the
network: it diffs the fork branch against the pinned upstream rev and replays the
checked-in patch to confirm it reproduces the fork tree exactly. Green here does not
substitute for that job.

**When it fires.** A stale sha means the fork branch moved without the pin (or the
patch) being re-exported — `git format-patch --stdout <upstream>..<fork-ref>`. Lines
under `discovery-core/src` mean the "consumed unmodified" sentence in the README and
spec has become false: revert the source change on the fork, or stop making the
claim. See `docs/ops/fork.md`.

## 6. Compose RPC defaults equal the code profile

**Protects** a host started without a `.env`: `docker-compose.yml` restates endpoints that
`crates/indexerd/src/config.rs` owns, and drift is silent — ingest stays healthy while every
anchor, `verify-root` and snapshot goes missing (issue #18; the Sepolia half of it was fixed by
hand in 61a790d).

**How.** The `${MAINNET_RPC_URL:-…}`, `${MAINNET_RPC_FALLBACK:-…}`, `${SEPOLIA_RPC_URL:-…}` and
`${SEPOLIA_RPC_FALLBACK:-…}` defaults must equal `MAINNET_RPC_PRIMARY`, `MAINNET_RPC_FALLBACK`,
`SEPOLIA_RPC_PRIMARY`, `SEPOLIA_RPC_FALLBACK`. A dropped `:-default` and a deleted variable fail
too — both leave the compose endpoint unpinned from the profile.

**When it fires.** Change the const, then copy it into compose (or the reverse); the failure
prints both values. Never delete the compose default to silence it.
