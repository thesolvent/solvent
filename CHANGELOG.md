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

- **ERC-7683 same-chain settlement (P2, tasks 2–3)** — the on-chain half of the second protocol.
  - **`SameChainSettler`** — a generic ERC-7683 v1 settler implementing both `IOriginSettler` and
    `IDestinationSettler` (on one chain they are the same contract). `openFor` verifies the swapper's
    signature, enforces `openDeadline`, consumes a replay nonce and escrows the input in a single
    Permit2 `permitWitnessTransferFrom`; `fill` takes the filler's output, pays the user, and releases
    the escrow in the same call, because on one chain proof-of-fill *is* the fill. `resolve`/`resolveFor`
    return the standard's `ResolvedCrossChainOrder`, so a third-party solver can price an order without
    a decoder. `originData` is bound to the escrow by hash, so a filler cannot substitute a cheaper order.
  - **`Erc7683AquaFiller`** — zero-inventory sourcing for a protocol that gives the filler no callback.
    The flash comes from SwapVM instead: with `isFirstTransferFromTaker` false the router pays the taker
    before taking payment, and `preTransferInCallback` fires *before* our input is pulled — so inside it
    we hold the maker's output having paid nothing, and call the settler there. Multi-leg sourcing nests
    (leg *i*'s callback launches leg *i+1*, the innermost settles, the stack unwinds paying makers
    outward), bounded at `MAX_LEGS = 4`, with the remaining plan carried in `preTransferInCallbackData`
    so no contract state spans the levels. Input allowances are granted at the plan's cumulative maximum
    up front rather than per leg, because a nested leg's revoke would strip the outer leg's allowance.
  - **`Erc7683FillBuilder`** — `RoutePlan` → `Erc7683AquaFiller.fill(...)` calldata, rejecting plans
    wider than `MAX_LEGS` off-chain rather than reverting on-chain (`FillBuilderError::TooManyLegs`).
  - **Cross-language pin closed** — `GenSolventOrderFixture.s.sol` emits a fixture from the real settler;
    the Rust codec's `orderId` and Permit2 witness digest are asserted against it, so the id the
    normalizer produces is the id the settler records.
  - **`RoutePlan::new`** — every other primitive had a constructor; `#[non_exhaustive]` otherwise left
    the type unconstructable outside `core`, which the fill builders need.
  - **23 Foundry tests** (14 settler, 9 filler) against source-deployed Permit2 + Aqua/SwapVM, including
    single-, two- and four-leg nested fills, the `MAX_LEGS` bound, callback authentication, and an
    under-sourcing case proving the settler cannot reach the filler's accrued spread. Every settler guard
    was mutation-checked: neutering it turns its test red.

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
- **ERC-7683 ingest (P2, task 1)** — a second protocol on the inbound edge, proving the adapter seam:
  the pipeline now consumes ERC-7683 v1 gasless orders alongside UniswapX ones with **no change to
  `IngestPipeline`, `Intent`, or any downstream slice**.
  - **Codec** — the standard's `GaslessCrossChainOrder` envelope plus the `SolventOrder` order type
    (fixed price + exclusivity window) it carries in `orderData`. Both are plain EIP-712 structs, so
    alloy derives the type strings and the `orderId` struct hash; nothing is hand-composed (unlike
    UniswapX, whose type string flattens `baseInput`). The two type strings are pinned by test — they
    are the cross-language contract with the settler that will record the same id.
  - **Normalizer** — envelope → canonical `Intent`, with fixed amounts on `AmountCurve::scalar` (no
    new curve code). Rejects a malformed payload, an `orderDataType` that is not ours, `orderData`
    that does not decode, and an `originChainId` disagreeing with the feed's chain.
  - **Order builder + `self_hosted` feed** — mints orders signed with a Permit2 EIP-712 witness over
    the envelope, composing the witness type string from alloy's derived type rather than a literal.
  - **`sign65` extracted** to `ingest/mod.rs` on its second use; the UniswapX builder now shares it.
  - Fill path and settler contract are deliberately **not** here — the `FillBuilder`'s first real
    consumer is the filler contract (plan task 3), and the `orderId` stays unpinned against Solidity
    until the settler exists (plan task 2). See `docs/plans/backend/erc7683-plan.md`.
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
- **Recapture (Tier 0) — arbitrage-by-design ★**: after same-direction flow leaves a maker's pool
  imbalanced, internalize the rebalancing (reverse) trade and rebate the recaptured LVR to that maker —
  value that otherwise leaks to an MEV searcher. Design:
  `docs/plans/backend/2026-09-05-arbitrage-recapture-design.md`.
  - **Pure split** (`primitives::recapture::recapture_split`) — per-leg LVR vs. the oracle mid
    (positive only on rebalancing legs, so forward/imbalancing fills self-exclude), taker-shared and
    capped by realized spread, aggregated per (maker, token); stateless, fail-closed, no imbalance ledger.
  - **Settlement seam** — `ExecutionService::reconcile` now reports `ConfirmedFill`s (fresh-post only,
    so a redelivery can't double-drive recapture); `RecaptureService` values the fill's *actual* legs —
    read back from its own Aqua `Pushed`/`Pulled` events via the `SettledLegsReader` port (Aqua adapter
    reusing the shared receipt decode), so a partial fill credits only what really moved — against the
    reused `PriceOracle`, and accrues the credits (best-effort — an unreadable settlement, missing price,
    or store error never fails the fill). Routing is unchanged: the cheap = best-priced preference
    already steers reverse flow into the imbalanced maker.
  - **Durable store** — `RecaptureStore` port + SQLite adapter (`recapture_credit`, keyed by
    (intent, maker, token) so a re-driven reconcile accrues once); `outstanding` / `mark_settled`.
  - **Payout** — `RebatePayer` port + alloy ERC-20 adapter; `PayoutService` sweeps outstanding credits,
    one transfer per (maker, token) summed across intents, settling a group only after its payment
    lands (never double-paid).
  - **Tests** — pure-split unit matrix; a hermetic walking skeleton (real `route` internalizes the
    reverse buy into the imbalanced maker → credit); store idempotency/settle; payout aggregation &
    failed-payment retry; a live anvil E2E paying a maker on chain and settling.
  - **Realized-spread cap** — the α cap now bounds rebates by the fill's *realized* resolver spread
    (the plan's expected spread adjusted by the actual-vs-plan sourcing drift), so a partial or
    dearer-than-planned fill never rebates past what it earned; a no-op on exact-in.
  - **Scope — internal-only (2026-09-06)**: Tier 1 public counter-intent auction, its ledger-race
    property tests, and the auction-set split are **out of scope** (that race rides on Tier 1; internal
    best-effort recapture is the disclosed limit, §12). The in-binary payout worker + config (§6.2/§8)
    and the oracle-staleness gate (§8/§9) are deferred as blocked on the app composition root / a caching
    price oracle. Cross-chain remains future work.

_Next: B6 — reconcile._

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
