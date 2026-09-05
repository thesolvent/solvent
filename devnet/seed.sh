#!/bin/sh
# Devnet seed: wait for the chain, then deploy the stack + Core-6 tokens.
# Runs once inside a foundry container; writes deployments/solvent-devnet.json (the manifest volume).
# Permit2/Multicall3 live at fixed canonical addresses and must be etched (a node cheat), not
# `new`-deployed — they land with execution in the next phase, so S1 doesn't place them.
set -e

# Idempotent: on a persistent chain the manifest already exists, so don't redeploy (which would
# churn addresses). A fresh chain (down -v) clears the volume and re-seeds.
if [ -f deployments/solvent-devnet.json ]; then
    echo "seed: already seeded (manifest present) — skipping"
    exit 0
fi

echo "seed: waiting for $RPC_URL ..."
until cast chain-id --rpc-url "$RPC_URL" >/dev/null 2>&1; do sleep 1; done

echo "seed: deploying stack + Core-6 tokens ..."
cd /work
forge script script/DeployDevnet.s.sol:DeployDevnet --broadcast --rpc-url "$RPC_URL" --private-key "$DEPLOYER_KEY"

echo "seed: done -> deployments/solvent-devnet.json"
