# Council — consumer path (A1–A6)

Three independent proposals were written against the same brief (snapshots +
storage-root anchor, SSE, the WASM package of Block B, the npm package,
`strk20-sync serve`, chain profiles, implementation order, acceptance legs),
each from a declared lens: [p1-verification.md](p1-verification.md)
(verification rigor and privacy), [p2-simplicity.md](p2-simplicity.md)
(operational simplicity), [p3-dx.md](p3-dx.md) (the wallet engineer's DX).
Three judges then scored every area independently — an auditor
(correctness/privacy invariants), a maintainer (five-year cost of correctness),
and an integrator (the person shipping the npm package next quarter) — ranking
proposals per area, mandating grafts and issuing vetoes. No proposal won
everywhere: P2 took A1, A2, A5 and the implementation order unanimously or
2-1; P1 took A6 unanimously and the acceptance-leg discipline 2-1; P3 took the
two browser-facing surfaces (A3 wasm ABI, A4 npm) 2-1 while supplying
indispensable grafts elsewhere and collecting the most vetoes. The synthesis
editor merged them into [../../../spec/consumer-path.md](../../../spec/consumer-path.md),
resolving every judge disagreement explicitly in its §0 — including the ones
decided against a judge majority on merit (the in-process DB transport is built
because the roadmap gives it, with per-fetch content-hash self-verification
answering the maintainer's objection; the wasm entropy parameter is kept to
preserve the empty-import purity gate, with nonce reuse made structurally
impossible by an authenticated counter) and the ones where a graft was
corrected rather than adopted (the snapshot anchor's offline proof walk ships,
but the claim that it grounds the basis block in canonical chain data does
not). The addendum's §0.3 carries the consolidated must-not-ship list.
