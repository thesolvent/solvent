#!/bin/zsh
# Bring up a complete local devnet without Docker: chain, contracts, backend, seed, trades.
#
# Everything lives under RUN_DIR so a cleaned /tmp only costs a rerun. Each step is skipped when
# its result is already present, so this is safe to run again after a crash.
set -e

REPO="$(cd "$(dirname "$0")/.." && pwd)"
RUN_DIR="${SOLVENT_RUN_DIR:-$REPO/devnet/generated/local}"
RPC="http://127.0.0.1:8545"
API_PORT="${SOLVENT_API_PORT:-8103}"
API="http://127.0.0.1:$API_PORT"
# Anvil's well-known account #0. Devnet only.
DEPLOYER=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80

mkdir -p "$RUN_DIR"
cd "$REPO"

step() { print -P "%F{green}▸%f $1"; }

# ── chain ────────────────────────────────────────────────────────────────────
if ! curl -s -o /dev/null --max-time 2 "$RPC"; then
  step "anvil on 8545"
  nohup anvil --host 127.0.0.1 --port 8545 --chain-id 31337 --block-time 1 \
    --disable-code-size-limit --slots-in-an-epoch 1 \
    --state "$RUN_DIR/anvil.json" --state-interval 5 \
    > "$RUN_DIR/anvil.log" 2>&1 &
  until curl -s -o /dev/null --max-time 1 "$RPC"; do sleep 1; done
else
  step "anvil already up"
fi

# ── contracts ────────────────────────────────────────────────────────────────
# The manifest can outlive the chain it describes, so probe the chain itself.
AQUA=$(python3 -c "import json;print(json.load(open('contracts/deployments/solvent-devnet.json'))['aqua'])" 2>/dev/null || echo "")
CODE=$([ -n "$AQUA" ] && cast code "$AQUA" --rpc-url "$RPC" 2>/dev/null || echo "0x")
if [ "$CODE" = "0x" ] || [ -z "$CODE" ]; then
  step "deploying contracts"
  # The registry cursor is persisted. A store from the previous chain points past the new chain's
  # head, and the watcher then fails every tick with `to_block must be >= from_block` while the
  # API keeps serving its frozen snapshot.
  rm -f devnet/generated/solvent.db devnet/generated/walletkit.redb
  (cd contracts && forge script script/DeployDevnet.s.sol:DeployDevnet \
     --broadcast --rpc-url "$RPC" --private-key "$DEPLOYER" > "$RUN_DIR/deploy.log" 2>&1)
else
  step "contracts already deployed"
fi

step "etching Permit2 + Multicall3"
(cd scripts && SOLVENT_RPC_URL="$RPC" pnpm run etch > "$RUN_DIR/etch.log" 2>&1)

# ── the filler's token allow-list ────────────────────────────────────────────
# DeployDevnet does not call setTokenAllowed, so without this every fill reverts TokenNotAllowed.
step "allow-listing tokens on the filler"
FILLER=$(python3 -c "import json;print(json.load(open('contracts/deployments/solvent-devnet.json'))['filler'])")
python3 -c "
import json
d=json.load(open('contracts/deployments/solvent-devnet.json'))
print('\n'.join(t['address'] for t in d['tokens'].values()))" | while read -r addr; do
  cast send "$FILLER" "setTokenAllowed(address,bool)" "$addr" true \
    --rpc-url "$RPC" --private-key "$DEPLOYER" > /dev/null 2>&1
done

# ── config + server ──────────────────────────────────────────────────────────
step "generating config"
(cd scripts && SOLVENT_RPC_URL="$RPC" SOLVENT_BIND_ADDR="127.0.0.1:$API_PORT" \
   pnpm run bootstrap > "$RUN_DIR/bootstrap.log" 2>&1)

if ! curl -s -o /dev/null --max-time 2 "$API/v1/config"; then
  step "backend on $API_PORT"
  source devnet/generated/env.sh
  export SOLVENT_CONFIG=solvent
  export RUST_LOG="${RUST_LOG:-solvent=info,solvent_core=info}"
  nohup ./target/debug/solvent > "$RUN_DIR/server.log" 2>&1 &
  until curl -s -o /dev/null --max-time 1 "$API/v1/config"; do sleep 1; done
else
  step "backend already up"
fi

# ── liquidity ────────────────────────────────────────────────────────────────
# One maker's two copies per pair are drained by ~2 trades, so seed three.
step "waiting for the price feed"
# Seeding a pair the feed has not priced yet ships a strategy without pushing balance, and Aqua
# then reverts SafeBalancesForTokenNotInActiveStrategy when the router routes to it.
for i in {1..40}; do
  PRICED=$(curl -s --max-time 4 "$API/v1/assets" | python3 -c "
import json,sys
try: items=json.load(sys.stdin)['result']['items']
except Exception: print(0); raise SystemExit
print(sum(1 for a in items if a.get('price_usd')))" 2>/dev/null || echo 0)
  TOTAL=$(curl -s --max-time 4 "$API/v1/assets" | python3 -c "
import json,sys
try: print(len(json.load(sys.stdin)['result']['items']))
except Exception: print(0)" 2>/dev/null || echo 0)
  [ "$TOTAL" -gt 0 ] && [ "$PRICED" = "$TOTAL" ] && break
  sleep 2
done
print "  $PRICED/$TOTAL assets priced"

step "seeding makers"
for KEY in \
  0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a \
  0x92db14e403b83dfe3df233f83dfa3a0d7096f21ca9b0d6d6b8d88b2b4ec1564e \
  0x4bbbf85ce3377467afe5d46f804f221813b2bb87f24d81f60f1fcdbf7cbf4356
do
  # A pair whose USD price the feed has not delivered yet fails that pair only; the rest of the
  # makers still carry the book, so a partial seed must not abort the run.
  (cd scripts && SOLVENT_RPC_URL="$RPC" SOLVENT_API_URL="$API" SOLVENT_SEED_MAKER_KEY="$KEY" \
     pnpm run seed >> "$RUN_DIR/seed.log" 2>&1) || print "  (partial seed for this maker)"
done

step "executing sample trades"
(cd scripts && SOLVENT_RPC_URL="$RPC" SOLVENT_API_URL="$API" \
   pnpm run trades > "$RUN_DIR/trades.log" 2>&1) || true
tail -30 "$RUN_DIR/trades.log"

print -P "\n%F{green}devnet up%f  rpc $RPC · api $API · logs $RUN_DIR"
print "frontend: echo VITE_API_BASE_URL=$API > fe/.env.local && (cd fe && pnpm dev)"
