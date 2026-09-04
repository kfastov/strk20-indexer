#!/usr/bin/env python3
"""The five invariants a reviewer keeps re-checking by hand. `scripts/check-invariants.py`

Python rather than bash for three reasons that each cost real effort in shell:
check 2 must ignore matches inside Rust comments (line-based `grep -v '//'` cannot
tell a doc comment from `//` inside a URL literal), check 4 is a set intersection
of harvested key material against every tracked file rather than a grep, and
check 1 compiles an allowlist parsed out of Rust source into path matchers.

Stdlib only (Python >= 3.9), no arguments, no config. Exit 0 unless something
FAILed; WARNs are for a human to eyeball. See docs/ops/invariants.md.
"""

import json, os, re, subprocess, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
R = lambda *p: os.path.join(ROOT, *p)
rel = lambda p: os.path.relpath(p, ROOT)
line_of = lambda src, off: src.count("\n", 0, off) + 1


def read(path, default=None):
    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            return f.read()
    except OSError:
        return default


def rs_files(*dirs):
    return sorted(os.path.join(b, n) for d in dirs for b, _, ns in os.walk(R(d))
                  for n in ns if n.endswith(".rs"))


def blank(src, strings=True):
    """Blank comments (and optionally string literals), preserving every offset."""
    out, i, n = list(src), 0, len(src)
    wipe = lambda a, b: [out.__setitem__(k, " ") for k in range(a, b) if out[k] != "\n"]
    while i < n:
        if src.startswith("//", i):
            j = src.find("\n", i)
            j = n if j < 0 else j
            wipe(i, j); i = j
        elif src.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if src.startswith("/*", j): depth, j = depth + 1, j + 2
                elif src.startswith("*/", j): depth, j = depth - 1, j + 2
                else: j += 1
            wipe(i, j); i = j
        elif src[i] == "r" and i + 1 < n and src[i + 1] in '#"':
            j = i + 1
            while j < n and src[j] == "#": j += 1
            if j < n and src[j] == '"':
                term = '"' + "#" * (j - i - 1)
                k = src.find(term, j + 1)
                k = n if k < 0 else k + len(term)
                if strings: wipe(i, k)
                i = k
            else:
                i += 1
        elif src[i] == '"':
            j = i + 1
            while j < n and src[j] != '"':
                j += 2 if src[j] == "\\" else 1
            j = min(j + 1, n)
            if strings: wipe(i, j)
            i = j
        else:
            i += 1
    return "".join(out)


def drop_test_mods(src):
    """Blank `#[cfg(test)] mod x { .. }` so test-only deps are not findings."""
    out = list(src)
    for m in re.finditer(r"#\[cfg\(test\)\]\s*(?:pub\s+)?mod\s+\w+\s*\{", src):
        depth, j = 1, m.end()
        while j < len(src) and depth:
            depth += (src[j] == "{") - (src[j] == "}")
            j += 1
        for k in range(m.start(), j):
            if out[k] != "\n": out[k] = " "
    return "".join(out)


# --- 1. no user-derived value in a feed URL -------------------------------
MARKERS = ("genesis.json", "manifest.json", "head.ndjson", "anchors.ndjson",
           ".strk20e.zst", ".strk20s.zst", ".anchor.json", "/live", "/feed/")
USER_DERIVED = re.compile(r"addr|address|owner|viewing|vkey|\bvk\b|secret|user|account|"
                          r"nullifier|pubkey|public_key|channel|commitment|leaf|slot|key", re.I)
# `get_bytes`/`get_optional` are the transport's only two fetch primitives.
FETCH = re.compile(r"(?:get_bytes|get_optional|(?:http|client)\s*\.\s*get)\s*\(\s*"
                   r"&?\s*(?:format!\s*\(\s*)?\"([^\"\n]*)\"")


def canon(lit):
    p = re.sub(r"\{[^{}]*\}", "{}", lit.strip())
    p = re.sub(r"^\{\}/", "", p).lstrip("/")   # a leading `{}/` is the feed base URL
    return "/" + p if p.startswith("feed/") else "/feed/" + p


def in_allowlist(lit, allow):
    """Whole paths must match a pattern exactly. A literal with no `/` is a
    fragment — a `dir.join(..)` filename or a `strip_suffix` test — and may only
    match the TAIL of an allowed pattern, never introduce a path component."""
    frag = re.sub(r"\{[^{}]*\}", "{}", lit.strip())
    if "/" in frag.strip("/"):
        return canon(lit) in allow
    return any(a.endswith(frag if frag.startswith(".") else "/" + frag) for a in allow)


def check_feed_urls():
    src = read(R("crates/e2e-tests/src/feed_urls.rs"))
    m = re.search(r"PATTERNS\s*:\s*\[&str;\s*\d+\]\s*=\s*\[(.*?)\];", src or "", re.S)
    if not m:
        return "FAIL", "cannot read the allowlist in crates/e2e-tests/src/feed_urls.rs", []
    allow = {canon(p) for p in re.findall(r'"([^"]+)"', m.group(1))}
    bad, seen = [], set()

    def judge(lit, where):
        if (where, lit) in seen:
            return
        seen.add((where, lit))
        hit = [n for n in re.findall(r"\{([A-Za-z_]\w*)[^{}]*\}", lit) if USER_DERIVED.search(n)]
        if hit:
            bad.append("%s  user-derived selector %s in feed path %r" % (where, hit, lit))
        elif "?" in lit or "&" in lit:
            bad.append("%s  query string on a feed path: %r" % (where, lit))
        elif not in_allowlist(lit, allow):
            bad.append("%s  %r -> %s is not in the closed allowlist" % (where, lit, canon(lit)))

    for f in rs_files("crates/client/src", "crates/consumer/src",
                      "crates/wasm/src", "crates/feed/src"):
        text = blank(read(f, ""), strings=False)
        at = lambda o: "%s:%d" % (rel(f), line_of(text, o))
        # (a) anything shaped like a feed artifact path, wherever it appears...
        for lm in re.finditer(r'"([^"\n]*)"', text):
            if " " not in lm.group(1) and any(k in lm.group(1) for k in MARKERS):
                judge(lm.group(1), at(lm.start()))
        # (b) ...plus anything handed to a fetch, artifact-shaped or not. This is
        #     what catches a NEW endpoint (`notes/{address}.json`) that (a) cannot.
        for cm in FETCH.finditer(text):
            judge(cm.group(1), at(cm.start()))
        for qm in re.finditer(r"\.\s*(query|query_pairs|set_query)\s*\(", blank(read(f, ""))):
            bad.append("%s  builds a query string on a request (%s)" % (at(qm.start()), qm.group(1)))
    return ("FAIL" if bad else "PASS"), \
        "%d path literals vs %d allowed patterns" % (len(seen), len(allow)), bad


# --- 2. Block B stays wasm-portable ---------------------------------------
BANNED = [(r"\brusqlite\b", "rusqlite"), (r"\btokio\b", "tokio"), (r"\breqwest\b", "reqwest"),
          (r"\bstd\s*::\s*fs\b", "std::fs"),
          (r"\bfs\s*::\s*(?:read|write|File|create|remove|metadata)", "fs::* filesystem call")]


def check_wasm_seam():
    bad, files = [], 0
    for f in rs_files("crates/consumer/src", "crates/wasm/src"):
        files += 1
        text = drop_test_mods(blank(read(f, "")))
        for pat, name in BANNED:
            for m in re.finditer(pat, text):
                bad.append("%s:%d  Block B references %s (breaks the browser build)"
                           % (rel(f), line_of(text, m.start()), name))
    manifests = ("crates/consumer/Cargo.toml", "crates/wasm/Cargo.toml")
    for c in manifests:
        sect = re.search(r"^\[dependencies\]\s*$(.*?)(?=^\[|\Z)", read(R(c), ""), re.S | re.M)
        for dep in ("rusqlite", "tokio", "reqwest"):
            if sect and re.search(r"^\s*%s\b" % dep, sect.group(1), re.M):
                bad.append("%s  [dependencies] pulls in %s" % (c, dep))
    return ("FAIL" if bad else "PASS"), \
        "%d files + %d manifests, %d host-API refs" % (files, len(manifests), len(bad)), bad


# --- 3. no silent truncation ----------------------------------------------
CHAIN_NOUN = re.compile(r"\b(events|blocks|logs|txs|receipts|page|chunk)\b", re.I)
PERSIST = re.compile(r"\b(commit|insert|write_all|flush|persist|fsync|execute_batch|save)\s*\(")


def check_truncation():
    warns, fails = [], []
    # The LIVE-8 guard, asserted by PRESENCE: getEvents windows must be single-page
    # and a continuation token in the reply must be refused loudly. Deleting this
    # is how the original data-dropping defect comes back.
    if not re.search(r"continuation_token\s*\.\s*is_some\s*\(\s*\)",
                     blank(read(R("crates/indexerd/src/ingest.rs"), ""))):
        fails.append("crates/indexerd/src/ingest.rs  the LIVE-8 guard "
                     "`page.continuation_token.is_some()` is gone — a truncated getEvents "
                     "page would now be accepted silently")
    for f in rs_files("crates/indexerd/src"):
        raw = read(f, "")
        text, lines = blank(raw), raw.splitlines()
        prod = drop_test_mods(text)      # a hit only in `text` sits inside a test mod
        at = lambda o: (rel(f), line_of(text, o), "(test) " * (prod[o] != text[o]),
                        lines[line_of(text, o) - 1].strip()[:86])
        for m in re.finditer(r"\.\s*take\s*\(", text):
            w = "%s:%d  %s.take( bounds: %s" % at(m.start())
            if CHAIN_NOUN.search(text[max(0, m.start() - 60):m.start()]) and \
                    os.path.basename(f) in ("ingest.rs", "rpc.rs"):
                fails.append(w + "   <- bounds chain data on the fetch path")
            else:
                warns.append(w)
        # "Unexplained": no comment justifying it and no visible guard above, i.e.
        # a bare exit from a loop rather than a stated termination condition.
        for m in re.finditer(r"\bbreak\b", text):
            ln = line_of(text, m.start())
            ctx = "\n".join(lines[max(0, ln - 9):ln - 1])
            if "//" not in ctx and not re.search(r"\b(if|while|match)\b", ctx):
                warns.append("%s:%d  %sunexplained break: %s" % at(m.start()))
        for m in re.finditer(r"(?:let\s+_\s*=\s*|\.\s*ok\s*\(\s*\)\s*;)", text):
            stmt = text[m.start():text.find(";", m.start()) + 1]
            (fails if PERSIST.search(stmt) else warns).append("%s:%d  %sdiscarded result: %s" % at(m.start()))
    status = "FAIL" if fails else ("WARN" if warns else "PASS")
    return status, "%d suspicious shapes (%d confident)" % (len(warns) + len(fails), len(fails)), fails + warns


# --- 4. no secrets in tracked files ---------------------------------------
# Field/var names whose VALUE is key material. `PUBLIC` vetoes: the pool address,
# the STRK token and class hashes are long hex too, and must never be harvested —
# doing so is what made the earlier hand-run of this check cry wolf.
SECRET_NAME = re.compile(r"private_?key|secret|viewing_?key|\bvk\b|seed|mnemonic|"
                         r"passphrase|password|signing_?key|api_?key", re.I)
PUBLIC_NAME = re.compile(r"public|address|class_hash|token|pool|block_hash|tx", re.I)
KEYFILE = re.compile(r"\.key$|\.hex$|keystore|(^|/)vk[_.\w]*\.txt$|viewing-key.*\.json$|"
                     r"accounts\.json$|(^|/)\.env(\.|$)")
SHAPES = [(re.compile(r"(private_?key|secret_?key|viewing_?key|mnemonic|passphrase)"
                      r"\s*[:=]\s*[\"']?(?:0x)?[0-9a-fA-F]{40,}", re.I), "secret-named long hex"),
          (re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----"), "PEM private key"),
          (re.compile(r"\b(?:AKIA[0-9A-Z]{16}|ghp_\w{36}|sk-[A-Za-z0-9]{24,})\b"), "provider token")]


def harvest_secrets():
    """Values from the GITIGNORED key locations only — never addresses or hashes."""
    roots = [R("data"), os.path.expanduser("~/.strk20"), R(".env")] + \
            [R("examples", d) for d in ("mainnet", "sepolia")]
    seen, out = set(), set()
    for root in roots:
        paths = [root] if os.path.isfile(root) else \
            [os.path.join(b, n) for b, _, ns in os.walk(root) for n in ns] if os.path.isdir(root) else []
        for p in paths:
            if p in seen or not KEYFILE.search(p) or os.path.getsize(p) > 1 << 20:
                continue
            seen.add(p)
            body = read(p, "")
            secret = lambda k: SECRET_NAME.search(k) and not PUBLIC_NAME.search(k)
            try:
                def walk(o, k=""):
                    if isinstance(o, dict):
                        for kk, v in o.items(): walk(v, kk)
                    elif isinstance(o, list):
                        for v in o: walk(v, k)
                    elif isinstance(o, str) and secret(k) and len(o) >= 32:
                        out.add(o.lower().removeprefix("0x"))
                walk(json.loads(body))
            except (ValueError, TypeError):
                if ".env" in p:
                    out |= {v.lower().removeprefix("0x") for k, v in
                            re.findall(r"^\s*(\w+)\s*=\s*[\"']?([^\"'\s]+)", body, re.M)
                            if secret(k) and len(v) >= 32}
                else:   # a bare .key/.hex/vk.txt file IS the secret
                    out |= {h.lower() for h in re.findall(r"(?:0x)?([0-9a-fA-F]{40,})", body)}
    return {v for v in out if len(v) >= 32}, len(seen)


def check_secrets():
    known, nkey = harvest_secrets()
    tracked = subprocess.run(["git", "-C", ROOT, "ls-files"], capture_output=True,
                             text=True).stdout.splitlines()
    me, bad, scanned = rel(os.path.abspath(__file__)), [], 0
    for t in tracked:
        if t == me or re.search(r"node_modules/|vendor/|\.(lock|png|jpg|gif|zst|db|wasm)$", t):
            continue
        body = read(R(t))
        if body is None or "\0" in body[:2048]:
            continue
        scanned += 1
        low = body.lower()
        bad += ["%s  contains a value from a gitignored key file (…%s)" % (t, k[-6:])
                for k in known if k in low]
        if not t.endswith("Cargo.lock"):
            bad += ["%s:%d  %s" % (t, line_of(body, m.start()), why)
                    for pat, why in SHAPES for m in [pat.search(body)] if m]
    return ("FAIL" if bad else "PASS"), \
        "%d tracked files vs %d values from %d key files" % (scanned, len(known), nkey), bad


# --- 5. upstream consumed unmodified --------------------------------------
# Scope, deliberately narrow (#17): this check answers only what the working
# tree can answer offline — does Cargo.toml still point discovery-core at OUR
# fork at a 40-hex pin, and is the checked-in patch still exactly one commit.
# Whether that commit's TREE equals upstream is
# .github/workflows/fork-delta-check.yml's job; it fetches both repos and
# diffs them, which is strictly stronger than re-deriving the same verdict
# from the patch text. Asserting it in both places only meant two places to
# update when the pin moved.
def check_fork():
    bad, pin = [], None
    sect = re.search(r'^\[patch\."[^"]+"\]\s*$(.*?)(?=^\[|\Z)', read(R("Cargo.toml"), ""), re.S | re.M)
    if not sect:
        bad.append("Cargo.toml  no [patch] section — the fork claim is unverifiable here")
    elif not (dep := re.search(r"^\s*discovery-core\s*=\s*(.+)$", sect.group(1), re.M)):
        bad.append("Cargo.toml  [patch] does not redirect discovery-core")
    else:
        git = re.search(r'git\s*=\s*"([^"]+)"', dep.group(1))
        pin = (re.search(r'rev\s*=\s*"([0-9a-f]{40})"', dep.group(1)) or [None, None])[1]
        if not git or "starkware-libs" in git.group(1):
            bad.append("Cargo.toml  [patch] does not point at our fork: %s" % dep.group(1).strip())
        if not pin:
            bad.append("Cargo.toml  [patch] discovery-core is not pinned to a 40-hex rev "
                       "(a branch/tag pin lets the fork move under us)")
    patch = read(R("patches/discovery-core-providers-gate.patch"))
    if patch is None:
        return "FAIL", "patch file absent", bad + ["patches/discovery-core-providers-gate.patch missing"]
    commits = re.findall(r"^From ([0-9a-f]{40}) ", patch, re.M)
    if len(commits) != 1:
        bad.append("patch carries %d commits; the claim is exactly one dependency-gating commit"
                   % len(commits))
    return ("FAIL" if bad else "PASS"), "fork pin %s, patch carries %d commit(s)" % (
        pin[:12] if pin else "MISSING", len(commits)), bad


CHECKS = [("1. feed URLs carry nothing user-derived", check_feed_urls),
          ("2. Block B stays wasm-portable", check_wasm_seam),
          ("3. no silent truncation in ingest/scan", check_truncation),
          ("4. no secrets in tracked files", check_secrets),
          ("5. upstream consumed unmodified", check_fork)]


def main():
    print("strk20 invariant checks — %s\n" % ROOT)
    results = [(fn() + (name,)) for name, fn in CHECKS]
    for status, note, _, name in results:
        print("[%-4s] %-40s %s" % (status, name, note))
    for status, _, details, name in results:
        if details:
            print("\n--- %s: %s ---" % (status, name))
            print("\n".join("  " + d for d in details))
    failed = any(s == "FAIL" for s, _, _, _ in results)
    print("\n" + ("FAILED — an invariant above is broken."
                  if failed else "OK — nothing broken (WARNs are for a human to read)."))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
