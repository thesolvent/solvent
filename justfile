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

# Export ABIs for the (future) backend to consume — stable artifacts, not out/.
abi:
    cd contracts && forge inspect src/UniswapXAquaFiller.sol:UniswapXAquaFiller abi --json > abi/UniswapXAquaFiller.json
    cd contracts && forge inspect src/DevToken.sol:DevToken abi --json > abi/DevToken.json

fmt:
    cd contracts && forge fmt

fmt-check:
    cd contracts && forge fmt --check

# The gate a contracts phase must pass before it closes.
gate: fmt-check build test

# --- backend (Rust) ---
be-fmt:
    cargo fmt

be-fmt-check:
    cargo fmt --check

be-clippy:
    cargo clippy --all-targets -- -D warnings
    cargo clippy --all-targets --no-default-features -- -D warnings
    cargo clippy --all-targets --all-features -- -D warnings

be-test:
    cargo test
    cargo test --no-default-features
    cargo test --all-features

# The gate a backend phase must pass before it closes.
be-gate: be-fmt-check be-clippy be-test
