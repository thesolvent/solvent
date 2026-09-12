#!/usr/bin/env sh
set -eu

# This is the strongest hermetic boundary available until the backend owns construction of the
# signed Compact claim, maker quote, and proof payloads. The Rust scenario proves every durable
# coordinator transition; the Foundry scenarios prove the corresponding calls move real assets
# without leaving resolver inventory in both native-output and ERC20-output paths.
cargo test -p solvent-adapters \
    --test e2e_crosschain_direct \
    direct_route_traverses_every_durable_backend_stage \
    -- --exact --nocapture

(
    cd contracts
    forge test --offline \
        --match-path test/CrossChainDirect.t.sol \
        --match-test test_case1_usesAquaOnBothChainsAndLeavesNoResolverInventory
    forge test --offline \
        --match-path test/CrossChainDirect.t.sol \
        --match-test test_case1_deliversDestinationErc20AndLeavesNoResolverInventory
)
