#!/usr/bin/env python3
"""Starknet Sepolia faucet: solve the PoW challenge and claim, over ONE pinned
TCP connection.

Two things the faucet API does not document, both learned the hard way:

  1. `/faucet/request` requires a `network` field. Without `network: "sepolia"`
     the request is rejected even though the challenge was issued for the same
     address.
  2. `POW_CHALLENGE_INVALID` is intermittent. With a fresh address and a freshly
     solved, verified 20-bit proof of work, five separate submissions were
     rejected and one succeeded under identical bodies. Reusing a single
     keep-alive TCP connection across `/pow/challenge` and `/faucet/request`
     succeeded on the first attempt — the API appears to be load-balanced across
     instances that do not share challenge storage, so pinning the connection
     pins the instance.

Grant is 5 STRK per address, with a per-address cooldown of ~24 h
(`429 ADDRESS_COOLDOWN`). Difficulty was 20 bits, solved in 0.4-1.2 s.

No secrets: the only argument is a public account address.

    python3 faucet.py 0x<address> [attempts]
"""
import hashlib
import http.client
import json
import sys
import time

HOST = "api.faucet.starknet.io"
PATH = "/api/public-agent"
NETWORK = "sepolia"  # required, undocumented

ADDR = sys.argv[1] if len(sys.argv) > 1 else sys.exit(__doc__)
ATTEMPTS = int(sys.argv[2]) if len(sys.argv) > 2 else 10


def leading_zero_bits(digest):
    n = 0
    for b in digest:
        if b == 0:
            n += 8
            continue
        return n + 8 - b.bit_length()
    return n


def solve(prefix, difficulty):
    p = prefix.encode()
    i = 0
    while True:
        if leading_zero_bits(hashlib.sha256(p + str(i).encode()).digest()) >= difficulty:
            return i
        i += 1


def req(conn, method, path, body=None):
    headers = {"Content-Type": "application/json", "Accept": "application/json", "Connection": "keep-alive"}
    conn.request(method, PATH + path, body=json.dumps(body).encode() if body is not None else None, headers=headers)
    r = conn.getresponse()
    raw = r.read()
    try:
        return r.status, json.loads(raw)
    except Exception:
        return r.status, raw.decode(errors="replace")[:200]


for attempt in range(1, ATTEMPTS + 1):
    conn = http.client.HTTPSConnection(HOST, timeout=30)  # pinned across both calls
    try:
        st, ch = req(conn, "POST", "/pow/challenge", {"userAddress": ADDR})
        if not (isinstance(ch, dict) and ch.get("status") == "success"):
            print(f"attempt {attempt}: challenge {st} {ch if not isinstance(ch, dict) else ch.get('code')}")
            if isinstance(ch, dict) and ch.get("code") == "ADDRESS_COOLDOWN":
                sys.exit(2)
            time.sleep(3)
            continue
        d = ch["data"]
        nonce = solve(d["powInputPrefix"], d["difficulty"])
        body = {
            "userAddress": d["userAddress"],
            "challengeId": d["challengeId"],
            "nonce": nonce,
            "network": NETWORK,
        }
        st, r = req(conn, "POST", "/faucet/request", body)
        print(f"attempt {attempt}: request {st} {json.dumps(r)[:200]}")
        if isinstance(r, dict) and r.get("status") == "success":
            rid = r["data"]["requestId"]
            for _ in range(40):
                time.sleep(5)
                st2, s = req(conn, "GET", f"/faucet/status/{rid}")
                print("  status:", st2, json.dumps(s)[:250])
                js = s.get("data", {}).get("jobStatus") if isinstance(s, dict) else None
                if js in ("confirmed", "failed"):
                    sys.exit(0 if js == "confirmed" else 3)
            sys.exit(4)
    finally:
        conn.close()
    time.sleep(3)

print("exhausted attempts")
sys.exit(1)
