# Changelog

All notable changes to Solvent are documented here. Format loosely follows
[Keep a Changelog](https://keepachangelog.com/) and [Semantic Versioning](https://semver.org/);
the project is pre-1.0 and evolving.

## [Unreleased]

### Added
- **`UniswapXAquaFiller`** — the P1 on-chain filler: a zero-inventory UniswapX taker that sources order
  outputs from makers' Aqua positions via the SwapVM router, executing an off-chain routing plan
  (`SourceSwap[]`). Supports multi-maker sourcing, multi-token outputs, and batched orders. Three safety
  layers (per-leg `amountInMaximum`, reactor approvals derived from the resolved orders, and a
  balance-snapshot profitability guard), plus `Ownable2Step` + a transient reentrancy guard.
- **Test suite** — 20 hermetic tests (happy paths, boundaries, every guard, admin, and a source-split
  fuzz) against source-deployed UniswapX + Aqua/SwapVM, plus an opt-in mainnet-fork test against the
  real V2 reactor + Permit2.
- **ABI export** — `abi/UniswapXAquaFiller.json` (via `just abi`) for the future backend.

_Next: the Rust backend phase._

## [0.1.0] — 2026-08-28

### Added
- **Monorepo scaffold** — `contracts/` (Foundry) + `docs/`; dev tooling (`justfile`,
  `.githooks/pre-commit`, CI) and repo maintenance artifacts (license, contributing, security,
  code of conduct, issue/PR templates).
- **Dependency foundation** — the 1inch Aqua/SwapVM stack pinned via npm (`package.json` +
  `yarn.lock`) and Uniswap UniswapX/permit2 pinned as git submodules (`foundry.lock`); the full
  graph compiles together under solc 0.8.30.
- **Design docs** — `SPEC.md` plus architecture, research, cross-chain, and flow-catalog deep dives
  describing the zero-inventory, thin-taker resolver model.
