#!/usr/bin/env bash
# 00 — one-time setup: vendor the Starknet Privacy SDK and install dependencies.
#
# The SDK is published on GitHub Packages, which needs a token with
# `read:packages` even for public packages. We do not use it. The upstream
# repository is public (Apache-2.0) and the `sdk/` workspace builds standalone,
# so we clone the pinned tag and build from source. No token, no registry auth.
#
# Why THIS tag: the mainnet pool class
# 0x67dddd89d80fedadc06b6f160798f94800a4a70164e5a24301cd0d6076b554d has an ABI
# that matches tags RC.3, RC.4 and RC.5 exactly (117/117 entries) and does NOT
# match RC.2 (5 differences, incl. a missing ClientAction/ServerAction variant)
# or RC.6 (8 differences). RC.5 is the newest exact match. See README.
set -euo pipefail

SDK_TAG="${STRK20_SDK_TAG:-PRIVACY-0.14.3-RC.5}"
SDK_REPO="${STRK20_SDK_REPO:-https://github.com/starkware-libs/starknet-privacy.git}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VENDOR="$HERE/vendor"
SRC="$VENDOR/starknet-privacy"
DEST="$VENDOR/starknet-privacy-sdk"

node_major="$(node -p 'process.versions.node.split(".")[0]')"
if [ "$node_major" -lt 24 ]; then
  echo "ERROR: Node >= 24 required (the SDK's OHTTP dependency needs modern WebCrypto); you have $(node --version)" >&2
  exit 1
fi

mkdir -p "$VENDOR"

if [ ! -d "$SRC/.git" ]; then
  echo "==> cloning $SDK_REPO"
  git clone --quiet "$SDK_REPO" "$SRC"
fi

echo "==> checking out $SDK_TAG"
git -C "$SRC" fetch --quiet --tags
git -C "$SRC" checkout --quiet "$SDK_TAG"
echo "    $(git -C "$SRC" log -1 --format='%H %ci')"

echo "==> building the SDK"
( cd "$SRC/sdk" && npm ci --no-audit --no-fund >/dev/null && npm run build >/dev/null )
sdk_version="$(node -p "require('$SRC/sdk/package.json').version")"
echo "    built @starkware-libs/starknet-privacy-sdk $sdk_version"

# Expose it under a stable path so package.json's file: reference never moves.
rm -rf "$DEST"
mkdir -p "$DEST"
cp -R "$SRC/sdk/dist" "$DEST/dist"
cp "$SRC/sdk/package.json" "$DEST/package.json"
cp -R "$SRC/sdk/node_modules" "$DEST/node_modules"

echo "==> installing example dependencies"
( cd "$HERE" && npm install --no-audit --no-fund >/dev/null )

echo
echo "Setup complete. SDK $sdk_version from $SDK_TAG."
echo "Next:  cp .env.example .env  &&  \$EDITOR .env  &&  set -a; . ./.env; set +a"
echo "Then:  node 01-create-account.mjs"
