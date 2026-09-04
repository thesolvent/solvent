# Changelog

All notable changes to Solvent are documented here. Format loosely follows
[Keep a Changelog](https://keepachangelog.com/) and [Semantic Versioning](https://semver.org/);
the project is pre-1.0 and evolving.

## [Unreleased]

### Added — backend (`crates/`, hexagonal Rust workspace)
- **B0 — foundations**: `core`/`adapters`/`app` workspace; shared primitives (typed ids,
  valuations, one public `SolventError`, read-only config) and the observability shim.
- **B1 — registry**: a live, event-sourced picture of maker Aqua liquidity that prices the three
  Tier-0 SwapVM curves closed-form.
  - Bit-exact curve engine (XYC, concentrated, pegged) with flat-fee + guard composition,
    differentially validated against on-chain `quote()` (360-vector corpus).
  - Aqua event model + curve-agnostic `apply()` fold; strategy/`Order` decoder via `sol!`;
    lock-free `ArcSwap` snapshot with a dual `StrategyKey`/`TokenPair` index; closed-form `price()`.
  - `ChainSource` port + alloy `eth_getLogs` adapter (vendored garden indexer); in-process SQLite
    event store (`sqlx`) behind a storage-agnostic `Store` port, with a per-chain cursor;
    `RegistrySync` — moka-deduped, DB-authoritative, cursor-last — with restart recovery.
  - Live E2E against anvil-deployed Aqua/SwapVM + a temp-file SQLite: full pricing matrix plus the
    watcher lifecycle (ship/push/swap/dock, duplicate re-scans, recovery, app-filter, multi-maker).
  - Deferred to later tasks: reorg handling, periodic multicall reconciliation, protocol/dynamic
    fees, control-flow jumps, Decay, Extruction.

_Next: implement `UniswapXAquaFiller.sol` + its hermetic/fork test matrix._

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
