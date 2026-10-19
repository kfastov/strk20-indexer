# Domain Docs

Use a **single-context** layout for this repo: one root `CONTEXT.md` and
`docs/adr/` shared by the Rust crates and TypeScript packages.

## Before exploring, read these

- **`CONTEXT.md`** at the repo root: the domain glossary.
- **`docs/adr/`**: read ADRs relevant to the area you are about to work in.

If these files do not exist, proceed silently. Do not flag their absence or
suggest creating them upfront. `/domain-modeling` creates them lazily when terms
or decisions are resolved. Routine implementation decisions remain in commit
messages, as described in `CLAUDE.md`.

## File structure

```text
/
├── CONTEXT.md
└── docs/
    └── adr/
        └── 0001-<decision>.md
```

`CONTEXT.md` is a glossary, not an implementation spec or scratchpad. Record an
ADR only for a hard-to-reverse choice that would surprise a future reader and
resulted from a real trade-off.

## Use the glossary's vocabulary

When naming a domain concept in an issue, refactor proposal, hypothesis, or test,
use the term defined in `CONTEXT.md`, including its preferred and avoided names.
If a needed concept is missing, check whether the project already has a term for
it; note a real glossary gap for `/domain-modeling`.

## Flag ADR conflicts

If a proposal contradicts an existing ADR, identify the ADR and explain why the
decision is worth reopening instead of silently overriding it.
