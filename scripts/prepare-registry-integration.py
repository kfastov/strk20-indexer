#!/usr/bin/env python3
"""Prepare an isolated CI checkout for testing registry-only SDK consumption."""

import argparse
import json
from pathlib import Path


PACKAGE = "@starkware-libs/starknet-privacy-sdk"
VERSION = "0.14.3-rc.5"
ROOT = Path(__file__).resolve().parent.parent


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--verify-locks", action="store_true")
    args = parser.parse_args()
    if args.verify_locks:
        for directory in ["ts", "examples/mainnet"]:
            lock = json.loads((ROOT / directory / "package-lock.json").read_text())
            upstream = lock["packages"]["node_modules/" + PACKAGE]
            assert upstream.get("version") == VERSION, upstream
            assert upstream["resolved"].startswith("https://npm.pkg.github.com/"), upstream
            assert upstream["integrity"].startswith("sha512-"), upstream
            assert not upstream.get("link"), upstream
            assert "vendor/" not in json.dumps(lock), "Stale vendor references in lockfile"
            print(json.dumps({"directory": directory, "upstream": upstream}))
        assert not (ROOT / "examples/mainnet/vendor").exists(), "Vendor must not exist"
        return

    for path in ["ts/strk20-discovery/package.json", "examples/mainnet/package.json"]:
        manifest = ROOT / path
        data = json.loads(manifest.read_text())
        data["dependencies"][PACKAGE] = VERSION
        manifest.write_text(json.dumps(data, indent=2) + "\n")

    # Exercise the existing bundled release format with the installed registry
    # package as its input. No upstream checkout or SDK build is involved.
    pack = ROOT / "ts/strk20-discovery/scripts/pack-release.mjs"
    original = pack.read_text()
    patched = original.replace(
        "resolve(repo, 'examples/mainnet/vendor/starknet-privacy-sdk')",
        "resolve(repo, 'ts/node_modules/@starkware-libs/starknet-privacy-sdk')",
    ).replace(
        "join(repo, 'examples/mainnet/vendor/starknet-privacy/LICENSE')",
        "join(repo, 'registry-access-results/upstream-LICENSE')",
    )
    assert patched != original and "examples/mainnet/vendor" not in patched
    pack.write_text(patched)


if __name__ == "__main__":
    main()
