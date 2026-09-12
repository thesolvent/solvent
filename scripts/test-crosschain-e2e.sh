#!/usr/bin/env sh
set -eu

run() {
    printf '\n==> %s\n' "$*"
    "$@"
}

run cargo test -p solvent-core --lib 'crosschain::'

run cargo test -p solvent-adapters --lib 'crosschain::'
run cargo test -p solvent-adapters --test crosschain_plan_validation
run cargo test -p solvent-adapters --test e2e_crosschain_direct
run cargo test -p solvent-adapters --test e2e_crosschain_matrix
run cargo test -p solvent-adapters --test e2e_crosschain_failures
run cargo test -p solvent-adapters --test e2e_crosschain_restart
run cargo test -p solvent-adapters --test verifier_crosschain_cctp_binding
run cargo test -p solvent-adapters --test verifier_crosschain_wrong_role
run cargo test -p solvent-adapters --test verifier_crosschain_proof_binding

(
    cd contracts
    run forge test --offline --match-path test/CrossChainDirect.t.sol
    run forge test --offline --match-path test/CrossChainRouted.t.sol
    run forge test --offline --match-path test/CcipProof.t.sol
)

(
    cd sdk
    run pnpm exec vitest run test/cross-chain/client.test.ts
)

printf '\nCross-chain E2E campaign passed.\n'
