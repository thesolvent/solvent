#!/bin/sh
# One-shot cross-chain contract deploy against the remote Coolify anvils, run via
# `scheduled_tasks.run_once` inside the deploy-tools container (built with this
# repo's contracts/ baked in, on the `coolify` docker network alongside anvil-origin
# and anvil-destination). Mirrors scripts/src/deploy/phases.ts's TS logic in shell
# since this container has no Node — only forge/cast.
set -e
cd /repo/contracts

ORIGIN_RPC="http://anvil-origin:8545"
DESTINATION_RPC="http://anvil-destination:8546"
# anvil listens on 8545 inside its own container regardless of the host-side port name.
DESTINATION_RPC="http://anvil-destination:8545"

DEPLOYER_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
DEPLOYER_ADDRESS=0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266
FILLER_OWNER_KEY=0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a
POLICY_SIGNER_KEY=0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6
DEVNET_DUMMY=0x000000000000000000000000000000000000dEaD

mkdir -p deployments/remote/origin deployments/remote/destination

phase1() {
  side=$1; rpc=$2
  out=deployments/remote/$side/devnet.json
  if [ -f "$out" ]; then echo "phase1[$side]: already deployed, skipping"; return; fi
  DEVNET_MANIFEST=$out FILLER_OWNER_KEY=$FILLER_OWNER_KEY POLICY_SIGNER_KEY=$POLICY_SIGNER_KEY \
    forge script script/DeployDevnet.s.sol:DeployDevnet --broadcast --rpc-url "$rpc" --private-key $DEPLOYER_KEY
}

phase2() {
  side=$1; rpc=$2; remote_selector=$3
  out=deployments/remote/$side/crosschain-infra.json
  if [ -f "$out" ]; then echo "phase2[$side]: already deployed, skipping"; return; fi
  CCIP_REMOTE_CHAIN_SELECTOR=$remote_selector CCIP_DESTINATION_CHAIN_SELECTOR=$remote_selector CROSSCHAIN_MANIFEST=$out \
    forge script script/DeployCrossChainInfra.s.sol:DeployCrossChainInfra --broadcast --rpc-url "$rpc" --private-key $DEPLOYER_KEY
}

echo "== phase1: origin =="
phase1 origin "$ORIGIN_RPC"
echo "== phase1: destination =="
phase1 destination "$DESTINATION_RPC"
echo "== phase2: origin (remote selector 22) =="
phase2 origin "$ORIGIN_RPC" 22
echo "== phase2: destination (remote selector 11) =="
phase2 destination "$DESTINATION_RPC" 11

echo "== phase3: origin compact =="
COMPACT_OUT=deployments/remote/origin/compact.json
if [ ! -f "$COMPACT_OUT" ]; then
  COMPACT_MANIFEST=$COMPACT_OUT forge script script/DeployCrossChainInfra.s.sol:DeployOriginCompact --broadcast --rpc-url "$ORIGIN_RPC" --private-key $DEPLOYER_KEY
else
  echo "phase3: already deployed, skipping"
fi

ORIGIN_AQUA=$(jq -r .aqua deployments/remote/origin/devnet.json)
ORIGIN_ROUTER=$(jq -r .router deployments/remote/origin/devnet.json)
ORIGIN_CHAIN_ID=$(jq -r .chain_id deployments/remote/origin/devnet.json)
ORIGIN_USDC=$(jq -r .tokens.USDC.address deployments/remote/origin/devnet.json)
ORIGIN_WETH=$(jq -r .tokens.WETH.address deployments/remote/origin/devnet.json)
ORIGIN_INBOX=$(jq -r .inbox deployments/remote/origin/crosschain-infra.json)
ORIGIN_OUTBOX=$(jq -r .outbox deployments/remote/origin/crosschain-infra.json)
ORIGIN_COMPACT=$(jq -r .compact "$COMPACT_OUT")

DEST_AQUA=$(jq -r .aqua deployments/remote/destination/devnet.json)
DEST_CHAIN_ID=$(jq -r .chain_id deployments/remote/destination/devnet.json)
DEST_USDC=$(jq -r .tokens.USDC.address deployments/remote/destination/devnet.json)
DEST_WETH=$(jq -r .tokens.WETH.address deployments/remote/destination/devnet.json)
DEST_INBOX=$(jq -r .inbox deployments/remote/destination/crosschain-infra.json)
DEST_OUTBOX=$(jq -r .outbox deployments/remote/destination/crosschain-infra.json)

echo "== phase4: cross-wired apps =="
SETTLER_OUT=deployments/remote/origin/settler.json
APP_OUT=deployments/remote/destination/app.json
if [ -f "$SETTLER_OUT" ] && [ -f "$APP_OUT" ]; then
  echo "phase4: already deployed, skipping"
else
  ORIGIN_NONCE=$(cast nonce $DEPLOYER_ADDRESS --rpc-url "$ORIGIN_RPC")
  DEST_NONCE=$(cast nonce $DEPLOYER_ADDRESS --rpc-url "$DESTINATION_RPC")
  PREDICTED_SETTLER=$(cast compute-address $DEPLOYER_ADDRESS --nonce $ORIGIN_NONCE | awk '{print $NF}')
  PREDICTED_APP=$(cast compute-address $DEPLOYER_ADDRESS --nonce $DEST_NONCE | awk '{print $NF}')
  echo "predicted origin settler: $PREDICTED_SETTLER"
  echo "predicted destination app: $PREDICTED_APP"

  DEVNET_DUMMY=$DEVNET_DUMMY \
    DESTINATION_AQUA=$DEST_AQUA DESTINATION_WETH=$DEST_WETH \
    ORIGIN_CHAIN_ID=$ORIGIN_CHAIN_ID ORIGIN_SETTLER=$PREDICTED_SETTLER ORIGIN_COMPACT=$ORIGIN_COMPACT \
    ORIGIN_TOKEN=$ORIGIN_USDC ORIGIN_USDC=$ORIGIN_USDC \
    DESTINATION_FILL_VERIFIER=$DEST_INBOX DESTINATION_PROOF_OUTBOX=$DEST_OUTBOX DESTINATION_REPAYMENT_VERIFIER=$DEST_INBOX \
    DESTINATION_USDC=$DEST_USDC APP_MANIFEST=$APP_OUT \
    forge script script/DeployCrossChainInfra.s.sol:DeployDestinationCrossChainApp --broadcast --rpc-url "$DESTINATION_RPC" --private-key $DEPLOYER_KEY

  APP_ADDR=$(jq -r .app "$APP_OUT")

  DEVNET_DUMMY=$DEVNET_DUMMY \
    ORIGIN_COMPACT=$ORIGIN_COMPACT ORIGIN_AQUA=$ORIGIN_AQUA ORIGIN_TOKEN=$ORIGIN_USDC \
    DESTINATION_CHAIN_ID=$DEST_CHAIN_ID DESTINATION_APP=$APP_ADDR \
    ORIGIN_FILL_VERIFIER=$ORIGIN_INBOX ORIGIN_PROOF_OUTBOX=$ORIGIN_OUTBOX ORIGIN_USDC=$ORIGIN_USDC \
    ORIGIN_SWAP_ROUTER=$ORIGIN_ROUTER DESTINATION_USDC=$DEST_USDC SETTLER_MANIFEST=$SETTLER_OUT \
    forge script script/DeployCrossChainInfra.s.sol:DeployOriginSettler --broadcast --rpc-url "$ORIGIN_RPC" --private-key $DEPLOYER_KEY
fi

echo "=== MANIFESTS ==="
echo "ORIGIN_DEVNET=$(cat deployments/remote/origin/devnet.json | tr -d '\n')"
echo "ORIGIN_INFRA=$(cat deployments/remote/origin/crosschain-infra.json | tr -d '\n')"
echo "ORIGIN_COMPACT_JSON=$(cat "$COMPACT_OUT" | tr -d '\n')"
echo "ORIGIN_SETTLER=$(cat "$SETTLER_OUT" | tr -d '\n')"
echo "DEST_DEVNET=$(cat deployments/remote/destination/devnet.json | tr -d '\n')"
echo "DEST_INFRA=$(cat deployments/remote/destination/crosschain-infra.json | tr -d '\n')"
echo "DEST_APP=$(cat "$APP_OUT" | tr -d '\n')"
echo "=== DONE ==="
