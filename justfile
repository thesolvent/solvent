# Solvent dev commands — run `just` to list.

default:
    @just --list

# --- contracts (Foundry) ---
# One-time after clone. Deps come from two upstream conventions, both pinned:
#   - 1inch Aqua/SwapVM ship as npm packages  -> yarn install (into node_modules/)
#   - Uniswap (UniswapX/permit2) ship as git submodules -> forge install (into lib/)
setup:
    cd contracts && yarn install --frozen-lockfile
    cd contracts && forge install

build:
    cd contracts && forge build

test:
    cd contracts && forge test

# Fork tests hit real mainnet contracts; needs MAINNET_RPC_URL in the env.
test-fork:
    cd contracts && forge test --fork-url mainnet

# Export the filler ABI for the (future) backend to consume — a stable artifact, not out/.
abi:
    cd contracts && forge inspect src/UniswapXAquaFiller.sol:UniswapXAquaFiller abi --json > abi/UniswapXAquaFiller.json

fmt:
    cd contracts && forge fmt

fmt-check:
    cd contracts && forge fmt --check

# The gate a contracts phase must pass before it closes.
gate: fmt-check build test

# --- backend (Rust) — recipes are wired when the backend phase begins ---
