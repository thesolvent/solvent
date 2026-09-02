# Changelog

All notable changes to Solvent are documented here. Format loosely follows
[Keep a Changelog](https://keepachangelog.com/) and [Semantic Versioning](https://semver.org/);
the project is pre-1.0 and evolving.

## [Unreleased]

### Added — contracts (`contracts/`, Foundry)
- **`UniswapXAquaFiller`** — the P1 on-chain filler: a zero-inventory UniswapX taker that sources order
  outputs from makers' Aqua positions via the SwapVM router, executing an off-chain routing plan
  (`SourceSwap[]`). Supports multi-maker sourcing, multi-token outputs, and batched orders. Three safety
  layers (per-leg `amountInMaximum`, reactor approvals derived from the resolved orders, and a
  balance-snapshot profitability guard), plus `Ownable2Step` + a transient reentrancy guard.
- **Test suite** — 20 hermetic tests (happy paths, boundaries, every guard, admin, and a source-split
  fuzz) against source-deployed UniswapX + Aqua/SwapVM, plus an opt-in mainnet-fork test against the
  real V2 reactor + Permit2.
- **ABI export** — `abi/UniswapXAquaFiller.json` (via `just abi`) for the future backend.

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
- **B2 — ledger ★**: an off-chain reservation + double-entry engine that makes an over-committable
  maker balance safe to promise across concurrent intents (the novel core).
  - Pure engine: two-ceiling atomic admission (shared wallet + strategy virtual), two-phase
    reserve / post (partial) / void / expire / void_reorg, conservation invariant (property-tested).
  - Single-writer `LedgerService` — durable-first (check → persist → commit) so the in-memory ledger
    never leads the store; lock-free `ArcSwap` of `available` for the quote path; TTL sweep; recovery.
  - `LedgerStore` (SQLite, idempotent, state-guarded transitions) + `BudgetSource` (chain
    `min(balanceOf, allowance)` in one Multicall3 + registry snapshot) + `Clock` ports.
  - Live E2E against anvil + SQLite: live-budget two-ceiling admission, shared-wallet & same-strategy
    races, firm-confirm on a budget drop, docked/unknown → zero budget, concurrent never-over-promise,
    mixed-state & idempotent recovery, partial/void/TTL lifecycle, multi-maker isolation.
  - Known limitations tracked in `docs/KNOWN_LIMITATIONS.md` (late-`Shipped` drift, reorg, and the
    performance items — snapshot clone, batched confirm — deferred to reconcile/reorg/optimization).

- **B3 — routing**: a capped water-fill solver that sources an intent across maker curves at the
  marginal-price optimum, gated on the taker's bound.
  - **Solver**: single-pair specialization of `CFMMRouter.jl`'s dual — bisect the water level λ
    where every active venue quotes the same net-of-fee marginal (Angeris' equimarginal
    principle); exact-in and exact-out, ε-validated against a brute-force reference both ways.
  - **Funnel + certificate**: rank candidates by estimated net-of-fee output at the trade size
    (what production routers select on), top-`k` via quickselect, with a conservative heuristic
    certificate that flags a too-small `k`.
  - **Gas-aware sparsity**: backward-elimination trading legs against a per-leg gas cost in the
    **spread token** (output for exact-in, input for exact-out) — `Split::net_output` /
    `gross_input`; `route()` gates profit symmetrically.
  - **Shared maker-wallet cap**: a maker's strategies share one wallet, so the solver caps their
    combined output with a per-maker KKT group-floor (`max(λ, λ*_g)`) — the plan stays
    simultaneously reservable (independently audited).
  - **Live gas model**: pure `per_leg_cost` (gas → native → USD → spread token) behind
    `GasPrice`/`PriceOracle` ports; adapters — lock-free `MarketCache`, periodic `GasPoller`, and
    a WebSocket `BinanceFeed` (bookTicker mid) — feed it off the quote hot path.
  - **Contract-fidelity hardening**: out-of-domain curve params rejected at decode; the pegged
    numerical fill degrades past the overflow sentinel; dust legs the chain reverts on are
    dropped; a leg's input is bounded to its output cap (no round-trip over-reservation).
  - **Validation (T5)**: an independent-oracle property suite (no reference solve — it would share
    the code under test) — Tier-0 invariants (conservation, order-independence, feasibility),
    Tier-1 optimality oracles (finite-difference KKT residual, round-trip, output-monotonicity),
    Tier-2 quality studies (2^K sparsity regret, funnel decomposition), Tier-3 extreme-scale
    robustness (1e6–1e28 reserves) — which surfaced and fixed four bugs a reference-solve test
    structurally cannot catch: exact-in under-spend, order-dependence, pegged FD-precision fill,
    and an unused-leg group-floor misflag.
  - Criterion latency benchmark (500 ms p99 budget) + a feature-gated quote-call counter, and a
    pricing matrix (scenarios A–H): baseline curve, depth sensitivity, fee-vs-curve separation,
    concentration/effective-depth, N-maker aggregation, heterogeneous split, wallet caps, and
    skew-vs-impact — the numbers behind the impact-vs-size characterization.
  - 3-component live E2E over anvil + SQLite — registry sync → `route` → ledger `reserve` —
    asserting every routed plan is reservable and the router declines beyond the ledger's caps.
  - Limitations and deferred test coverage tracked in `docs/KNOWN_LIMITATIONS.md`.

_Next: B4._

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
