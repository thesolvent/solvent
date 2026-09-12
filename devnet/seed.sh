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

echo "seed: installing canonical Permit2 and Multicall3 runtimes ..."
PERMIT2=0x000000000022D473030F116dDEE9F6B43aC78BA3
MULTICALL3=0xcA11bde05977b3631167028862bE2a173976CA11
PERMIT2_RUNTIME=$(tr -d '\n' < /permit2_runtime.hex)
cast rpc anvil_setCode "$PERMIT2" "$PERMIT2_RUNTIME" --rpc-url "$RPC_URL" >/dev/null
cd /work
MULTICALL3_RUNTIME=$(forge inspect src/DevMulticall3.sol:DevMulticall3 deployedBytecode)
cast rpc anvil_setCode "$MULTICALL3" "$MULTICALL3_RUNTIME" --rpc-url "$RPC_URL" >/dev/null

echo "seed: deploying stack + Core-6 tokens ..."
forge script script/DeployDevnet.s.sol:DeployDevnet --broadcast --rpc-url "$RPC_URL" --private-key "$DEPLOYER_KEY"

echo "seed: done -> deployments/solvent-devnet.json"
