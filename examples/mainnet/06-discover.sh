#!/usr/bin/env bash
# 06 — close the loop: run OUR OWN client against OUR OWN feed and let it find
# the notes the previous steps created, using only the viewing key.
#
# This is the point of the whole exercise. Steps 03-05 put real notes on
# mainnet; this step proves that strk20-sync discovers them from a published
# feed, with the viewing key never leaving this process and no privileged
# access to the pool.
#
# Requires: STRK20_FEED (an http(s) /feed endpoint from `strk20 run`, or a local
# mirror directory produced by `strk20 backfill`).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"

KEYSTORE="${STRK20_KEYSTORE:-$HOME/.strk20/mainnet-keystore}"
KEYSTORE="${KEYSTORE/#\~/$HOME}"          # expand a leading ~
ACCOUNT_JSON="$KEYSTORE/account.json"
KEY_FILE="$KEYSTORE/viewing-key.hex"
DB="${STRK20_SYNC_DB:-$KEYSTORE/sync.db}"
DB="${DB/#\~/$HOME}"
FEED="${STRK20_FEED:-}"

if [ ! -f "$ACCOUNT_JSON" ]; then
  echo "ERROR: no keystore at $ACCOUNT_JSON — run 01-create-account.mjs first" >&2
  exit 1
fi
if [ ! -f "$KEY_FILE" ]; then
  echo "ERROR: no viewing key at $KEY_FILE" >&2
  exit 1
fi
if [ -z "$FEED" ]; then
  cat >&2 <<'EOS'
ERROR: STRK20_FEED is not set.

  It must point at a feed this project produced. Either:

    a local mirror directory
      strk20 backfill --db ./mainnet.db --feed-dir ./mainnet-feed
      export STRK20_FEED=./mainnet-feed

    or a running feed server
      strk20 run --db ./mainnet.db --feed-dir ./mainnet-feed
      export STRK20_FEED=http://127.0.0.1:8080/feed

  A full mainnet backfill takes roughly an hour and produces a ~16 MB feed.
EOS
  exit 1
fi

ADDRESS="$(node -p "require('$ACCOUNT_JSON').account_address")"

# Build the client if it is not already on PATH.
if command -v strk20-sync >/dev/null 2>&1; then
  SYNC=(strk20-sync)
else
  echo "==> building strk20-sync (release)"
  ( cd "$REPO" && cargo build --release -p strk20-client --bin strk20-sync )
  SYNC=("$REPO/target/release/strk20-sync")
fi

echo
echo "==> discovering notes for $ADDRESS"
echo "    feed:        $FEED"
echo "    mirror db:   $DB"
echo "    viewing key: $KEY_FILE  (read by the client, never printed, never sent)"
echo

# --network mainnet makes the client refuse a feed stamped with any other chain
# id, so a Sepolia feed cannot silently produce an empty (and misleading) result.
exec "${SYNC[@]}" sync \
  --feed "$FEED" \
  --address "$ADDRESS" \
  --key-file "$KEY_FILE" \
  --db "$DB" \
  --network mainnet \
  "$@"
