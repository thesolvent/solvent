# Solvent

[![CI](https://github.com/21r21a33333/solvent/actions/workflows/ci.yml/badge.svg)](https://github.com/21r21a33333/solvent/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Solidity](https://img.shields.io/badge/Solidity-0.8.30-363636.svg)](https://soliditylang.org/)
[![Foundry](https://img.shields.io/badge/Built%20with-Foundry-1e1e1e.svg)](https://getfoundry.sh/)

> A **zero-inventory, protocol-agnostic intent resolver** on **1inch Aqua**. Solvent fills intent orders
> (starting with UniswapX) using non-custodial **maker** liquidity borrowed through Aqua *at the instant
> of settlement* — holding no capital of its own, keeping the spread.

A single Aqua-based maker balance backs solver inventory across many protocols at once (1inch, UniswapX,
Across, CoW…), instead of each protocol needing its own siloed inventory. The maker's capital stays
non-custodial and does double duty; Solvent is a thin **taker** of the audited 1inch SwapVM router and
the UniswapX reactor — it re-implements neither pricing nor settlement.

Built for **ETHOnline 2026**.

## Read in this order

1. **[`SPEC.md`](SPEC.md)** — the bible. Start here; it's self-contained (~10 min read).
2. [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — Phase 1 (UniswapX) architecture in full detail.
3. [`docs/RESEARCH.md`](docs/RESEARCH.md) — the critical research + honest novelty analysis.
4. [`docs/CROSS_CHAIN_SETTLEMENT_PRIMER.md`](docs/CROSS_CHAIN_SETTLEMENT_PRIMER.md) — cross-chain settlement from first principles.
5. [`docs/CROSS_CHAIN_OPERATIONS.md`](docs/CROSS_CHAIN_OPERATIONS.md) — the implemented two-service proxy architecture, lifecycle, and runbook.
6. [`docs/RESOLVER_FLOW_CATALOG.md`](docs/RESOLVER_FLOW_CATALOG.md) — every protocol flow + component inventory.

Diagrams live in [`docs/images/`](docs/images/).

## Repo layout

```
contracts/   Foundry — the on-chain filler (UniswapXAquaFiller) + mainnet-fork tests
docs/        spec, research, diagrams
justfile     dev commands (run `just`)   ·   .githooks/  fmt + comment gate   ·   .github/  CI + templates
```

The **Rust backend** crate is added in its own phase (its layout is decided then).

## Getting started

Prerequisites: [Foundry](https://getfoundry.sh/), [Node.js](https://nodejs.org/) + [Yarn](https://classic.yarnpkg.com/), and [`just`](https://github.com/casey/just) (optional — a task runner; each recipe is a plain command you can also run by hand).

```sh
just setup     # fetch pinned deps (yarn -> node_modules/, forge -> lib/)
just build     # forge build
just test      # hermetic tests
MAINNET_RPC_URL=… just test-fork   # opt-in mainnet-fork tests
```

Dependencies follow each upstream's own convention, both pinned and reproducible: **1inch Aqua/SwapVM**
as npm packages (`yarn`, locked by `yarn.lock`) and **Uniswap UniswapX/permit2** as git submodules
(`forge install`, locked by `foundry.lock`). No external paths.

Enable the git hooks once: `git config core.hooksPath .githooks`.

## Contributing & security

See [`CONTRIBUTING.md`](CONTRIBUTING.md) and [`SECURITY.md`](SECURITY.md). By participating you agree to
the [Code of Conduct](CODE_OF_CONDUCT.md).

## License

[MIT](LICENSE).
