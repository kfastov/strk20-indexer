#!/usr/bin/env python3
"""Test the real npm registry with isolated configuration and caches."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import urllib.error
import urllib.request


PACKAGE = "@starkware-libs/starknet-privacy-sdk"
VERSION = "0.14.3-rc.5"
ENCODED = "@starkware-libs%2fstarknet-privacy-sdk"
GITHUB = "https://npm.pkg.github.com"


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--expect", choices=["allowed", "denied"], required=True)
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    token = os.environ.get("NODE_AUTH_TOKEN", "")
    assert token, "The probe requires the workflow's own token"
    report = {"package": PACKAGE, "version": VERSION, "expected": args.expect, "checks": []}

    def redact(text):
        return text.replace(token, "[REDACTED]")

    def record(name, **fields):
        item = {"check": name, **fields}
        report["checks"].append(item)
        print(json.dumps(item), flush=True)
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2) + "\n")

    def metadata(name, base, authenticated):
        headers = {"Accept": "application/json", "User-Agent": "strk20-registry-probe"}
        if authenticated:
            headers["Authorization"] = "Bearer " + token
        req = urllib.request.Request(base + "/" + ENCODED, headers=headers)
        try:
            with urllib.request.build_opener(NoRedirect).open(req, timeout=20) as response:
                data = json.load(response)
                version = data.get("versions", {}).get(VERSION)
                record(name, status=response.status, version_present=version is not None,
                       integrity=version.get("dist", {}).get("integrity") if version else None)
                return response.status
        except urllib.error.HTTPError as error:
            detail = redact(error.read(4096).decode("utf-8", errors="replace"))
            record(name, status=error.code, detail=detail)
            return error.code
        except Exception as error:
            record(name, error=redact(str(error)))
            raise

    metadata("npmjs-anonymous", "https://registry.npmjs.org", False)
    metadata("github-anonymous", GITHUB, False)
    status = metadata("github-workflow-token", GITHUB, True)
    with tempfile.TemporaryDirectory(prefix="strk20-registry-probe-") as temp:
        root = Path(temp)
        (root / "package.json").write_text(json.dumps({
            "name": "strk20-registry-probe", "version": "1.0.0", "private": True,
            "type": "module", "dependencies": {PACKAGE: VERSION},
        }))
        (root / ".npmrc").write_text(
            "@starkware-libs:registry=" + GITHUB + "\n"
            "//npm.pkg.github.com/:_authToken=${NODE_AUTH_TOKEN}\n"
        )
        for name in ["user.npmrc", "global.npmrc"]:
            (root / name).write_text("")
        env = {k: v for k, v in os.environ.items() if k in {"PATH", "LANG", "LC_ALL", "TMPDIR"}}
        env["NODE_AUTH_TOKEN"] = token
        common = ["--ignore-scripts", "--no-audit", "--no-fund", "--fetch-retries=0",
                  "--fetch-timeout=20000", "--loglevel=error", "--userconfig", str(root / "user.npmrc"),
                  "--globalconfig", str(root / "global.npmrc")]

        def run(name, command):
            result = subprocess.run(command, cwd=root, env=env, capture_output=True,
                                    text=True, timeout=180)
            record(name, exit_code=result.returncode,
                   stdout=redact(result.stdout[-4000:]), stderr=redact(result.stderr[-6000:]))
            return result.returncode

        install = run("npm-install", ["npm", "install", "--cache", str(root / "install-cache"), *common])
        if args.expect == "denied":
            assert status in (401, 403) and install != 0, "Negative control unexpectedly had access"
            return
        assert status == 200 and install == 0, "Workflow token cannot install the official SDK"
        lock = root / "package-lock.json"
        original_lock = hashlib.sha256(lock.read_bytes()).hexdigest()
        assert run("npm-ci-fresh-cache", ["npm", "ci", "--cache", str(root / "ci-cache"), *common]) == 0
        assert hashlib.sha256(lock.read_bytes()).hexdigest() == original_lock, "npm ci changed the lockfile"
        record("lockfile-unchanged", sha256=original_lock)
        code = """
import assert from 'node:assert/strict';
import * as sdk from '@starkware-libs/starknet-privacy-sdk';
import * as testing from '@starkware-libs/starknet-privacy-sdk/testing';
for (const name of ['Witness', 'AddressMap', 'Channel', 'createPrivateTransfers'])
  assert.ok(name in sdk, 'Missing SDK export: ' + name);
for (const name of ['MockProofInvocationFactory', 'compute_note_id'])
  assert.ok(name in testing, 'Missing testing export: ' + name);
console.log('Required runtime and testing exports are available');
"""
        assert run("runtime-exports", ["node", "--input-type=module", "-e", code]) == 0
        record("complete", installed_version=VERSION)


if __name__ == "__main__":
    main()
