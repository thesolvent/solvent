#!/bin/sh
# Devnet acceptance smoke — run after `just devnet-up`. Hits the host-exposed ports.
set -e
RPC=http://127.0.0.1:8545
FAUCET=http://127.0.0.1:8080
EXPLORER=http://127.0.0.1:5100

echo "smoke: chain id = $(cast chain-id --rpc-url $RPC)"

FINALIZED=$(cast block finalized --rpc-url $RPC --field number 2>/dev/null || echo 0)
echo "smoke: finalized block = $FINALIZED"
[ "$FINALIZED" -gt 0 ] || { echo "FAIL: finalized tag not advancing (fills would never confirm)"; exit 1; }

echo "smoke: faucet health = $(curl -fsS $FAUCET/health)"

echo "smoke: faucet drip ->"
curl -fsS -X POST $FAUCET/faucet -H 'content-type: application/json' \
  -d '{"address":"0x70997970C51812dc3A010C7d01b50e0d17dc79C8"}' | head -c 600
echo

echo "smoke: explorer http = $(curl -fsS -o /dev/null -w '%{http_code}' $EXPLORER/)"
echo "smoke: ok"
