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
- **`DevToken`** — a mintable ERC-20 with configurable decimals, for seeding the devnet with
  real-world-decimal test tokens.
- **`DeployDevnet` script** — one-shot deploy of Aqua + the SwapVM router + the reactor + filler +
  the Core-6 tokens, writing an address manifest (`solvent-devnet.json`).

### Added — devnet (`devnet/`, Docker Compose)
- **Self-contained local devnet** — one `docker compose up` (`just devnet-up`) boots anvil (fast
  finality via `--slots-in-an-epoch 1`, oversized code allowed, `--state` persistence), Otterscan, a
  one-shot idempotent deploy of the full stack + Core-6 test tokens, and a rate-limited token/gas
  **faucet** (`crates/devnet`, receipt-verified mints + per-address cooldown). No wharfnet runtime
  dependency; a Coolify deploy guide is included.

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

- **B4 — ingest**: the inbound edge that turns an off-chain UniswapX order into a clean, validated,
  deduped canonical `Intent` and fills it on-chain through the resolver's executor.
  - **Canonical `Intent`** — protocol-agnostic (opaque `raw` + `ProtocolId`, typed common fields);
    `AmountCurve` (static / linear Dutch decay) priced by `amount_at`, rounding by slope direction to
    match on-chain decay bit-for-bit; models both exact-in and exact-out orders. `#[non_exhaustive]`
    throughout so new protocols/fields don't break callers.
  - **UniswapX V2 normalizer** — decodes the real `V2DutchOrder` and reproduces `V2DutchOrderLib.hash`
    (the `orderHash`) from the contract's flattened EIP-712 type strings + alloy `abi_encode`
    (differentially tested against the contract); maps cosigner overrides + exclusivity, rejecting
    orders whose cosigner-override arity the reactor would revert on.
  - **Ingest pipeline** — `OrderFeed`/`Normalizer` ports; fan-in → normalize → admit (chain, deadline,
    amounts) → `moka` TTL dedup → bounded `mpsc` (backpressure). Off-chain cosignature/fillability left
    to the reactor.
  - **Order builder + dual signature** — mints signed+cosigned V2 orders our reactor accepts: swapper
    Permit2 EIP-712 witness + cosigner raw digest (never `sign_message`); digests differentially
    tested against the contract. `self_hosted` feed streams them (the `hosted` poll feed and `replay`
    are deferred — see `docs/KNOWN_LIMITATIONS.md`).
  - **Fill builder** — `RoutePlan` → `UniswapXAquaFiller.fill(...)` calldata (`Bytes`); the registry
    retains each strategy's shipped program to source from.
  - **Full-loop live E2E** over anvil — a self-hosted order → normalize → route → reserve → **on-chain
    fill** through the deployed reactor + filler + etched Permit2, sourcing the output from a shipped
    Aqua maker; both signatures verify on-chain.
- **B5 — execution**: closes the intent lifecycle — a reserved `RoutePlan` becomes an included fill,
  and the outcome is coupled back to the ledger. A thin wrapper over `walletkit`: it owns fill
  semantics, walletkit owns the tx lifecycle (sign / private-submit / track / bump / reorg / nonce).
  - **`Execution` + `SimGate` ports** — solvent-native `FillTx`/`ExecStatus`/`SimVerdict` so core never
    depends on walletkit; the `WalletkitExecutor` adapter wraps one `Wallet`, implementing both:
    submits on a construction-fixed route (private relay in prod, public on a local chain), simulates
    via `dry_run`, and projects walletkit's eight tx states onto the terminal signal the ledger needs.
  - **Fill-aware sim gate** — eth-calls the exact fill before a nonce is spent; the filler's on-chain
    guards (under-delivery / profit threshold / stale caps) surface as reverts, so a clean simulation
    proves all of them at once. Fail-closed. (A revm fork-sim can slot in behind the same port later.)
  - **`SettlementReader` port + Aqua adapter** — reads the *actual* per-source amounts a confirmed fill
    pulled from its own receipt's Aqua `Pulled` events (shared `IAqua` decode with the registry), so
    the ledger posts what truly happened and returns any unfilled remainder — never the reserved hold.
  - **`ExecutionService`** — `fill` (sim → submit, voiding the reservation on a reject before any
    nonce) and `reconcile` (tick → post the actual amount on confirm, void on failure); idempotent by
    intent, one-shot terminal transitions guarded by the ledger FSM, with an `on_reorg` compensation
    hook. Confirmation is finality-anchored (walletkit).
  - **Full-loop live E2E** over anvil through the **production execution path** — order → … → reserve
    → sim → submit → confirm → post the actual pulled amount; plus a stale-order sim-reject → void.
- **S2 · M0 — HTTP API scaffold**: the app becomes a running axum server (was `fn main(){}`),
  exposing the read foundation of the product API.
  - Inbound HTTP adapter (`crates/adapters/http`): the garden-rs `Response<T>` envelope, `SolventError`
    → HTTP mapping (redacted 5xx, real cause logged), `List<T>` + opaque cursor pagination, and a
    tower-http middleware stack (request-id, trace, timeout, CORS).
  - Composition root (`crates/app`): TOML config (`config` crate), registry snapshot hydrated once
    from the durable log, a background chain-head poller (block number cached — no RPC per request),
    graceful shutdown.
  - **`AssetManager`** — one authority answering everything about an asset by composing a
    Uniswap-shape token list with the live snapshot (`supported` / count / pairs); one rich `Asset`,
    serialized directly. `Snapshot::active_assets()` defines "supported".
  - Endpoints: `GET /healthz`, `/v1/config`, `/v1/stats`, `/v1/assets` (`?supported`), and a
    code-generated `/v1/openapi.json` (utoipa).

_Next: S2 M1 — discovery read paths (pools, pool detail, wallet balances)._

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
