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

- **S2 · M1 — discovery read paths**: the Pools list, Pool-detail, and Swap/Create balance screens
  render against a live server.
  - Endpoints: `GET /v1/pools` (list; symmetric `token_a`/`token_b` + `type`/`fee` filters, most-liquid
    first), `/v1/pools/detail?base&quote` (KPIs + maker roster), `/v1/pools/depth?base&quote&side`
    (executable-liquidity curve), `/v1/wallets/{addr}/balances` (per-token balance + pullable across
    the whole catalog).
  - **Pool read-surface** — `Snapshot::pool_stats()` folds active strategies per pair (maker count,
    spread band, popular fee tier, curve mix) with `itertools` grouping; `PoolService` composes it
    with the `AssetManager` for labels + Stable/Correlated/Volatile classification. Detail composes
    the list row (`#[serde(flatten)]`) plus the roster.
  - **Depth = the router, plotted** — reuses candidate `select` + the water-fill `solve`, swept by
    target output across an impact-anchored ladder (0.1–10%, bisected per bucket). No new curve math;
    impact is exact via `Ratio::rel_diff_bps`.
  - **Shared budget cache** — one synced `ArcSwap<AvailableSnapshot>` of every active maker's
    executable cap (`min(pullable wallet, registry virtual)`), refreshed off the request path by a
    supervised poller (batched — two Multicall3 aggregates for the whole book); depth reads it
    lock-free, no per-request RPC. (The router's quote path converges on one net-of-reservations
    snapshot in M2.)
  - **Wallet balances** — a `BalancesOracle` returning *both* balance and pullable (unlike
    `BudgetSource`'s `min`), read on demand for an arbitrary wallet in two Multicall3 aggregates; a
    shared `erc20` read helper backs both the budget source and the oracle.
  - Money crosses the wire as `Amount { raw, display, usd }` — exact base-unit strings, never floats.
  - **Live-run dependency:** Multicall3 predeployed on the devnet (the S1 coordination item); the
    depth/balances chain reads are otherwise unit- and (for the shared reader) anvil-E2E-tested.

- **S2 · M2 — the core swap loop**: the write path and the lifecycle read surface — a signed order is
  quoted, submitted, reserved, filled, confirmed on-chain, settled, and observable, restart-safe end
  to end.
  - **Trade store** — a durable, idempotent trade lifecycle: `TradeStore` port (create dedups on the
    order hash, `advance` monotonic by status rank, `settle` terminal-guarded by `settled_at`, `info`,
    filtered/paginated `list`, `find_by_order`, aggregate `stats`); `TradeId` (ULID, time-sortable);
    a normalized `trade`/`trade_leg`/`trade_attempt` schema (SQLite).
  - **`POST /v1/swap/quote`** — exact-in quote over one `select` + `solve_sparse` pass (no double
    solve); best-price impact read straight off the routed candidates; per-leg gas priced from the
    poller-backed cache (zero RPC on the path).
  - **`POST /v1/swap`** — the submit path: a taker-signed UniswapX V2 order is verified and cosigned
    (`ServerCosigner`, keys env-only, no `Debug` leak), normalized, routed **exact-out**, and driven
    create → reserve → fill; idempotent on the order hash. Request shaped like the UniswapX Orders API.
  - **Durable execution recovery** — the in-flight set is no longer in memory: each submitted fill's
    `(order_hash → reservation, engine handle)` is persisted behind a `FillStore` port (SQLite), and
    walletkit runs on a durable redb store, so a restart recovers and reconciles every in-flight fill
    through the normal reconcile tick — no separate recovery path. Kill-and-restart E2E.
  - **Reconcile worker** — a supervised loop that drives in-flight fills to terminal, settles the
    matching trade (confirmed / failed), and TTL-sweeps orphaned holds *excluding* still-in-flight
    reservations (so a hold is never released while its tx can land); a swept orphan fails its trade.
  - **`GET /v1/trades` + `/v1/trades/{id}`** — one `Trade` wire DTO (list omits the heavy
    lifecycle/legs/order fields, detail fills them); status/taker/pair filters, keyset cursor paging.
  - **`GET /v1/activity`** — the Aqua event feed (ship/push/pull/dock) from the durable log, keyset
    paged, with kind/actor/token filters; `aqua_event` gains a clock-stamped `created_at`, backing a
    real `events_24h`.
  - **`GET /v1/stats`** filled — `events_24h`, `trades_settled`, `confirmed_pct` (over confirmed +
    failed), `median_impact_pct`, and `active_makers` / `quoting_now` from the live registry.
  - **Phase-close refactor** (from a footprint + comment audit): token-decimals and per-leg gas cost
    de-duplicated across the quote/swap paths (`AssetManager::decimals`, a shared `LegCostResolver`);
    price impact now computed once in routing and produced onto settled trades; fixes for a
    crash-between-create-and-reserve wedge, decline-stat consistency, and activity pagination; a
    codebase-wide comment trim to the house standard.

- **S3 · M3 — USD valuation**: a core `Valuation` service over the existing `PriceOracle`
  (`usd(amount, token)` / `tvl(iter)`, missing price → `None`, never a fabricated zero), the
  `BinanceFeed` tracked symbols widened from the routing set to all Core-6 assets (+ 24h change from
  the ticker stream), and the `$`/`change` fields wired across every M1/M2/M4 DTO (`Amount.usd`, pool
  TVL, trade impact-$, stats volume/fees).
- **S3 · M4 — maker dashboard & analytics** (routes 12–16): the maker read-surface.
  - **Range decoder + metrics** — `sqrt_price→human` range labels beside the curve engine; a
    `quote_events` capture on `POST /swap/quote` and a `MakerMetricsStore` (SQLite rollups: fills,
    volume, fees, uptime, latency-p50, fill-share) grouped by maker/strategy/day.
  - **`GET /v1/makers`** (active roster) + **`GET /v1/makers/{maker}`** — the dashboard: headline
    KPIs with period-over-period `*_change_pct`, market-share fill-share over the maker's pairs, and a
    "cheaper competitor" insight; `me` resolves to the caller's wallet.
  - **`GET /v1/makers/{maker}/inventory`** — per-token rows (wallet / shared / fees / APY) with the
    contributing legs, transposed from the position set.
  - **`GET /v1/makers/{maker}/trades`** — the settlement feed, scoped to `trade_leg.maker`, with
    per-fill `share_pct` and `fee_usd` computed from **trade-time** token prices (persisted on the
    trade, so a settled fee never drifts with the market).
  - **`GET /v1/positions/{hash}`** + **`GET /v1/makers/{maker}/positions`** — the canonical `Position`
    (human `range`, `balances{virtual, actual, backed, coverage, opening, split}`, `economics`,
    detail-only `active_stats`), the list projection omitting detail fields; opening balances read from
    the event log.
- **S3 · M5 — write path: `@solvent/sdk` + thin backend** (polyglot; routes 17–18): a **client-side,
  non-custodial** TypeScript SDK plus the two endpoints it needs.
  - **`@solvent/sdk`** (new in-repo pnpm package: tsup dual ESM/CJS, vitest, `sideEffects:false`,
    per-module subpath exports) — hexagonal-lite: a pure core + one HTTP seam.
    - **`construction`** — a fluent `Strategy` builder (`fullRange`/`concentrated`/`inRange`/`pegged`
      `.fee(bps).build(maker)` → `{program, strategyHash, order}`) that reuses the `@1inch/swap-vm-sdk`
      price/band primitives (decimals-aware `Price`, `linearWidthFromSymmetricRangePercent`); encoding
      round-tripped against the shared decoder corpora the Rust side also validates.
    - **`positions`** — `positions({aqua, app})` → `approve`/`ship`/`dock`/`push`, each an unsigned
      `{to, data, value}` for the maker's own wallet (the SDK holds no key, sends nothing); `ship`/
      `dock` via `@1inch/aqua-sdk`, `push`/`approve` via viem + the shipped ABIs.
    - **`client`** — `createSolventClient({baseUrl, transport?, headers?})`, one typed method per
      route over an injectable `Transport` (defaults to `fetch`), throwing `SolventApiError` /
      `SolventNetworkError`; wire types generated from the OpenAPI snapshot.
  - **`GET /v1/pairs`** — Create-wizard candidate pairs (every asset quoted against a stable, plus
    stable/stable) with kind, mid, defaults, and optional per-side wallet balances; `?search=` filter.
  - **`POST /v1/positions/preview`** — server-authoritative pre-flight for an SDK-encoded ship:
    `{exists, requires_approval, warnings}`, where `requires_approval` is the allowance-capped
    `pullable < amount`.
  - **OpenAPI single-source-of-truth** — a committed `sdk/openapi.json` snapshot with a drift guard on
    each side (a backend test vs `ApiDoc::openapi()`, and the SDK's `codegen:check` vs the generated
    types), so a renamed Rust field surfaces as a compile/gate failure, never a runtime one.

  - **Live UniswapX order feed** — `OrdersApiClient` polls the public Orders API (unauthenticated,
    no pagination, `limit` clamped at 50) and `HostedFeed` alternates two scopes — the open book and
    orders reserved to this filler — over one request budget. Point `orders_api_url` at a local
    mirror to replay recorded pages offline; omit it to leave the feed off and take orders only from
    `POST /v1/swap`. Until now the ingest pipeline was built and connected to nothing.
  - **`Admission`** — the intake gate: supported chains, a token allow-list, an output-leg cap, and
    rejections for native-ETH legs (the reactor pays those from its own pre-funded balance, which an
    ERC-20 filler cannot source) and for outputs spanning several tokens. The dedup cache is bounded
    as well as expiring, and is consulted after those rules rather than before.
  - **Order validation in `UniswapXV2Normalizer`** — now stateful, holding the reactor it fills for
    and the cosigner keys it trusts. Recovers and pins the cosignature (the reactor checks only that
    a signature matches the order's own `cosigner` field, so pinning the identity is ours to do),
    pins the reactor, requires `deadline >= decayEndTime`, bounds the cosigner's overrides, and
    refuses any order carrying an `additionalValidationContract` — an arbitrary contract the reactor
    would call inside our fill, able to burn unbounded gas or revert on state a simulation cannot
    reproduce.
  - **`DecisionService`** — holds fed orders and re-prices each one every tick until it becomes
    fillable, its deadline passes, or somebody else takes it. Re-pricing is in-memory curve maths and
    free; committing is not, and the write path assumes a single attempt per order in three separate
    places (trade dedup on the order hash, a reservation id that stays claimed after voiding, and
    per-intent idempotent submission) — so the loop evaluates many times and acts once, after which
    the order leaves the service. `standing()` separates an expired order from one merely barred by
    another filler's window, which lapses.
  - **`Intent::required_output(filler, at)`** — what the settler will actually demand: every output
    leg summed at one instant, each raised by the exclusivity toll when the window belongs to someone
    else. `None` for a strict window (`overrideBps == 0`), which bars everyone else at any price.
  - **Real-order test corpus** — four captured mainnet Dutch V2 orders covering native output,
    multi-output, decaying input and both exclusivity states, each carrying the `orderHash` Uniswap
    published so the `V2DutchOrderLib` hash port is checked against their arithmetic. Plus an
    end-to-end suite driving the whole loop — Orders API stand-in, production client, validation,
    the loop declining while unaffordable, then filling against a source-deployed reactor and Aqua,
    through to reconciliation. Ticks are driven by hand, so the waiting is an assertion, not a sleep.

  - **Registry watcher start block and scan span** (`registry_start_block`, `registry_scan_span`) —
    the watcher can begin at the Aqua deployment instead of genesis, in chunks a node will actually
    serve. Both were hardcoded, which made a cold sync against mainnet thousands of `getLogs` calls
    over history that cannot hold an event, at a span dense enough to exceed a node's log cap.
  - **Deployed-router pricing parity** (`crates/core/tests/mainnet_fork_parity.rs`) — real mainnet
    strategies with their real Aqua balances, checked against the amounts the deployed SwapVM
    router returns for them. The oracle is the production contract rather than another transcription
    of it, so a divergence fails a test instead of reverting a fill. Strategies carrying the
    unimplemented Aqua protocol fee are asserted to be *declined* rather than priced: skipping that
    instruction over-quotes by up to 0.025% and the reactor refuses an amount we cannot source.
  - **Mainnet-fork profile for the maker seeder** (`scripts/src/seed/fork.ts`) — ships positions
    through the same SDK path a real maker uses, funding real tokens instead of minting DevTokens,
    with per-position prices so a deliberately mispriced maker can be shipped on purpose. Handles two
    mainnet quirks the devnet tokens never exercise: UNI rejects an allowance above `uint96`, and
    USDT reverts on changing a non-zero allowance.

  - **Readiness reflects the order feed** (`GET /readyz`) — `503` once the feed has been unreachable
    past its silence budget, `200` otherwise. `/healthz` stays a liveness constant: a stale feed
    should drain traffic and page someone, not restart a process whose problem is upstream. The
    health type existed but was read by nothing, so the failure it was written for — a filler that
    sees no orders while looking healthy — was live.
  - **1inch Limit Order Protocol feed** — `OneInchFeed`/`OneInchNormalizer` poll 1inch's Orderbook
    API and normalize plain, `ALLOW_MULTIPLE_FILLS`, and `FeeTaker`-gated orders into the same
    `Intent` pipeline UniswapX orders use, so they are admitted or dropped and quote-priced the
    same way, and — now that `OneInchFillBuilder` is wired in — actually filled when profitable.
    An order whose amount depends on unrecovered extension bytecode (a predicate, a runtime amount
    calculator, Permit2) is treated as malformed and never logged, the same as an unparseable
    UniswapX order. Off by default; set `oneinch_orderbook_url`, `oneinch_filler`, and
    `ONEINCH_API_KEY` to enable. Independent of UniswapX's own `orders_api_url` in both directions —
    either feed, both, or neither can be configured, and the ingest pipeline and decision loop now
    start whenever at least one is (previously they only ever started under UniswapX's own key,
    which would have silently left 1inch inert in a 1inch-only deployment).
  - **`OneInchFillBuilder` and `OneInchLimitOrderAquaFiller`** — the P2 on-chain filler: a
    zero-inventory 1inch Limit Order Protocol taker that sources a fill's taker-asset leg from
    makers' Aqua positions via the SwapVM router, the same sourcing mechanism `UniswapXAquaFiller`
    uses behind a different callback shape (1inch's own `ITakerInteraction` taker-interaction hook,
    fired between the protocol's maker→taker and taker→maker transfers, rather than a reactor
    calling back into the filler). `takerInteraction` is otherwise reachable by anyone naming this
    contract as the interaction target on an unrelated order of their own, so the callback checks
    both a per-fill transient flag and the hash of the specific order `fill()` is mid-call on — a
    same-tx flag alone is not enough, since `order.makerAsset`/`takerAsset` are maker-chosen and
    unvalidated, and a malicious token's transfer hook can call the real protocol on a second,
    attacker-crafted order that also names this contract before our own fill returns. Hermetic
    Foundry suite (7 tests: single- and multi-maker sourcing, the profitability guard's callers,
    both auth guards, admin) plus an opt-in mainnet-fork test against the real deployed 1inch
    Aggregation Router V6, both against a source-deployed/real 1inch Limit Order Protocol +
    Aqua/SwapVM. A hermetic Rust E2E (`e2e_oneinch_fill`) drives a signed order through the real
    `IngestPipeline`/`Admission`/`OneInchNormalizer`/`DecisionService`/`SwapService`/
    `OneInchFillBuilder` against source-deployed contracts, catching a real bug the Solidity suite
    couldn't: 1inch's `Address`/`MakerTraits` custom value types canonicalize to `uint256` in a
    function selector (not `address`), so the fill-builder's calldata was targeting the wrong
    selector entirely — fixed in `fill.rs`.
  - **`FillBuilder::build` now returns `BuiltFill { target, calldata }`** instead of bare calldata —
    `SwapService` no longer assumes one shared filler-contract address (`SwapConfig.filler` dropped
    that meaning; it's kept only for the UniswapX-specific exclusivity-toll check, which no other
    protocol's intents carry). `main.rs` registers both `UniswapXFillBuilder` and, only once
    `oneinch_filler` is actually set to a real deployment, `OneInchFillBuilder` — left out
    entirely while unset (there is no real mainnet deployment yet), since a call to an address with
    no code would otherwise simulate as a trivial on-chain success and broadcast a real,
    gas-spending transaction that settles nothing, rather than declining cleanly.
  - **1inch orders needing an epoch-manager check are no longer refused** — `NEED_CHECK_EPOCH_MANAGER`
    only gates *whether* an order can still be filled (a soft-cancellation the real protocol
    enforces on-chain), it never changes the maker/taker amounts, so it is treated the same as a
    `FeeTaker` whitelist rejection: accepted at its real stated amounts, left to decline at
    execution time if the epoch has moved on. Verified against real live orderbook data: every
    liquid-pair order this codebase currently has liquidity for (UNI/USDT, USDC/USDT, USDT/USDC)
    carries this bit, so refusing it outright meant refusing real, sourceable flow for no reason.
  - **A second UniswapX feed: `Limit`-type orders, the original `ExclusiveDutchOrderReactor`** —
    a separate deployment and order struct (`ExclusiveDutchOrder`, `ProtocolId::UniswapXV1`) from
    the V2 reactor's `Dutch_V2` feed, wired up as its own independent feed/normalizer pair,
    `uniswapx_v1_orders_api_url`/`uniswapx_v1_reactor`. Reuses the existing `UniswapXFillBuilder`
    and deployed `UniswapXAquaFiller` — both only ever forward `intent.settler`/`raw`/`signature`
    opaquely, with no reactor-generation-specific logic to duplicate. The wire format was pinned
    against a real order pulled from the live API rather than assumed from the plain
    (non-exclusive) `DutchOrderLib` docs, which are two fields short of what the reactor this
    order type actually serves carries (`exclusiveFiller`/`exclusivityOverrideBps`) — decoding the
    assumed shape against real data failed outright until corrected. A second real-data mismatch:
    a fully flat order (no price movement, `decayStartTime == decayEndTime`) is common in live
    `Limit` flow, and the real `DutchDecayLib.decay` special-cases `startAmount == endAmount`
    before it ever looks at the window — a flat leg never reverts on `EndTimeBeforeStartTime`, even
    with a zero-width window. The normalizer initially rejected these outright; fixed to only
    require a valid window for a leg that actually decays, mirroring the contract's own check
    order, verified against real fetched orders that were being wrongly refused.
    Proven end to end on a mainnet fork: `UniswapXV1AquaFillerFork.t.sol` fills a real order
    against the real deployed `ExclusiveDutchOrderReactor` through the same policy-authorized
    `UniswapXAquaFiller` contract — no contract change needed for this reactor generation.
  - **The order-feed pipeline and decision loop now start whenever at least one feed is
    configured** (UniswapX V2, UniswapX V1, or 1inch), not only under UniswapX V2's own key —
    previously a deployment running only the 1inch or V1 feed would have left the entire ingest
    pipeline inert despite looking fully configured.
- **The order feed records why a trade declined, and `/v1/orders` exposes it end to end.**
  `Settlement`/`Trade`/`TradeView` carry a `decline_reason` (`crates/adapters/migrations/
  0018_trade_decline_reason.sql`), populated with the real admission rule, the sim gate's actual
  on-chain revert reason, or the margin call — not just the terminal status. A `settle()`'s `reason:
  None` leaves an already-recorded reason as-is (`COALESCE` on write), so the reconcile loop's own
  terminal settle can't clobber the reason the swap service set at creation. Fixed a real decode bug
  along the way: `row_to_trade`'s source match only recognized `"uniswapx"`, silently mapping every
  `"oneinch"` trade to `OrderSource::Solvent`.
- **`GET /v1/orders` gained real pagination and filtering.** `offset`/`total` replace the previous
  fixed 200-row window — a deployment with heavier 1inch volume was pushing genuinely-filled
  UniswapX orders out of the only window the API could return. Filtering is server-side, not
  client-side: `source`, `token_in`/`token_out` (a directional pair), and `state` (a derived bucket
  — `filled`/`declined`/`failed_onchain`/`unprofitable`/`refused`/`pricing` — built from verdict
  plus trade lifecycle the same way the explorer UI computes it, not a stored column) all narrow
  both the page and its `total`, so a filtered page count is never wrong. The feed's `trade.id`
  is exposed too, so a filled order can link straight to its trade's own detail page.

### Added — frontend (`fe/`, React + Vite)
- **The order feed is a full peer of Trades/Activity**, with real protocol icons (sourced from
  CoinGecko, not fabricated), a source badge whose hover tooltip names the actual order type
  (e.g. "UniswapX — Dutch-auction intent order (V2 reactor)"), and a state pill whose hover tooltip
  carries the real reason (an admission rule, a decline reason, an on-chain revert) instead of
  showing it as a permanently-visible line. Filters for source, pair, and state are backed by the
  new server-side `/v1/orders` query params, so a filtered result's page count is always correct
  rather than only ever reflecting one page's worth of client-side filtering. Ten rows per page,
  with a `‹ page / total ›` control sitting beside the summary line. A row that became a trade
  (has a `trade_id`) routes straight to its trade detail page on click; a row with nothing to route
  to (refused, or not yet a trade) stays a plain row.
- **Live Makers and strategy details** — connect the original dashboard and strategy panels to
  address-based reads, rolling maker periods, confirmed-order fill share and submission-to-confirmation
  latency. Keep the existing chart/control placement, restore the prior Explorer/Trade layout,
  and use strategy-hash URLs for existing entity links and the green route transition.
- **Strategy price history** — derive fee-free marginal prices from each Aqua program and committed
  reserves, replay complete transactions with block timestamps, and retain gaps before creation or
  after docking. Header reads use a bounded cache and skip individual timestamps before the window.
- **Maker analytics windows** — apply settlement periods before pagination, retain historical
  volume/fee estimates and pair fill share after docking, and retain meaningful fractional token
  quantities. Shared-liquidity change remains unavailable until deposit/dock history is valued.
- **Accurate maker settlement pages** — one maker supplying several strategies appears once per
  trade, with combined exact token amounts and correctly aggregated share/fees. The trades API
  accepts `strategy_hash`, inventory legs expose their strategy hash, and generated SDK types
  reflect both additions. Owner/trade indexes support the filtered reads.
- **Internal navigation shares the green route transition** — pool and Explorer trade links,
  breadcrumbs, header actions, redirects, and browser Back/Forward all reveal their destination
  behind the same sweep. Rapid navigation cancels stale timers; reduced motion opens immediately.
  Accepted swaps navigate to the returned trade ID, while submission failures stay on Swap and
  late acceptance preserves a requested departure. Trade detail keeps tracking settlement.
- **Live Explorer and trade detail** — trades, protocol activity, and aggregate stats come from the
  API through an Explorer port, DTO mappers, and TanStack Query. Server filters and cursor paging
  replace fixture filtering. Trade URLs use stable IDs (`/explorer/trades/:tradeId`), with recorded
  lifecycle times, maker legs, and deployment-specific transaction/address links. Pending details
  refresh until terminal; missing data stays unknown, and non-confirmed output is labelled as the
  signed minimum. Two Playwright checks exercise the views against a running devnet.
- **Live refresh and settlement progress** — pending trade detail refreshes every two seconds and
  stops periodic polling at terminal states. Recorded stages animate as they arrive, with an awaited
  stage indicator and reduced-motion support. Explorer, activity, stats, pool data and assets refresh
  every five seconds while visible and on focus/reconnect; timestamps distinguish fresh, delayed,
  and paused updates. Pool detail now lists actual confirmed settlements for both pair directions.
- **Explorer maintainability** — lifecycle progress is a semantic model rendered by a focused
  component; CSS owns layout and hover, without app-wide pointer state. Query errors stay in the
  service layer. Stage fills preserve staggered timing and respect reduced motion regardless of
  shared stylesheet order. Duplicate missing-trade coverage is consolidated.
- **S4 · pools — the list and detail views, wired end to end.** The frontend gains the same seam the
  backend has: `ports/` (the domain types views speak), `adapters/{http,mappers}` (OpenAPI DTO →
  domain), `services/` (TanStack Query hooks), `lib/` (view-model math), `views/` (as migrated from
  the design mock). Services never import an adapter; the composition root injects them.
  - **Pools list** — server-fed rows, with fee-tier / APR / pool-type / TVL filters whose options are
    drawn from the pools actually present, and a recommendation pinned to the best APR.
  - **Pool detail** at its own address (`/pools/:pair`) — aggregated depth plotted from the depth
    endpoint, a maker roster with a **Virtual / Actual** toggle, and impact tiers that put the marker
    and its readout on the curve while hovered.
- **S4 · swap — the widget priced by the server.** The output amount, price impact and fill count
  come from `POST /v1/swap/quote` rather than a mid-price estimate, so what is shown includes fee
  and impact. The quote paces its own refresh off the expiry the server issued, re-prices on
  return to the tab, and re-polls a failure so an outage heals without retyping.
  - **Pair-constrained pickers** — the output leg offers only assets the input one is quotable
    against (which also makes picking the same asset twice impossible), and an asset in no pair is
    not offered at all. Both legs always name a pair the deployment actually serves.
  - **The action button carries the reason** — a size with no route is disabled and says so in the
    server's own words, returning to `Swap` when a size is fillable.
  - **Filter vocabularies read off the served assets**, so no tag or chain is offered that matches
    nothing.
- **`@solvent/scripts`** — a devnet runbook package: manifest bootstrap, a Multicall3 etch, idempotent
  strategy seeding priced off the server's own oracle, and an endpoint smoke matrix.
  - **`starter`** seeds eight distinct pairs, executes 24 sample trades covering both directions,
    and checks the read API. Scripts verify the devnet deployment before minting or signing, reuse
    SDK approval/signing, and save a local report counting only receipt-verified confirmations.
    Node uses the supported CommonJS SDK/viem entry points without a custom module loader.
  - Seed commands have explicit setup, funding, execution and reporting stages, with inert imports
    and one narrow SDK/viem runtime bridge. Native TypeScript scripts require Node 22.18+ (22.x)
    or 24.2+; frontend and SDK runtime requirements are unchanged.

### Changed — backend (`crates/`)
- **`Intent` is built from a named `IntentParts`** rather than eleven positional arguments, two of
  which were adjacent bare addresses (`swapper` and `settler`) that no test would catch transposed.
  It also now carries `swapper`: without it, feed-driven fills produce no trade rows.
- **Trade responses expose stored price impact** for Explorer list and detail. OpenAPI and SDK types
  carry the optional value; the existing `deadline_block` field is documented as Unix seconds.
- **Pools carry `tvl_change_24h_pct`** — the value-weighted 24h move of what the pool holds,
  all-or-nothing across its tokens, so a partial reading can never understate it.
- **Pool makers carry `actual`** — the committed amount capped by what Aqua may actually pull, so a
  roster can show deliverable size beside committed size. `None` when the chain read failed, which is
  not the same as nothing being deliverable.

### Fixed — backend (`crates/`)
- **Cosigner amount overrides were unasserted.** They replace the swapper-signed start amounts, and
  the captured corpus carries them, but nothing failed if they were ignored: the synthetic builder
  only ever emits zero overrides, and the corpus tests asserted hashes and shape rather than
  amounts. Ignoring them under-sources by 0.22% on one captured order — a revert after every maker
  leg is bought — and overstates receipts by 0.18% on another, which prices a losing trade as
  profitable.
- **The taker's input as the routing max-in bound was unasserted.** Replacing it with `U256::MAX`
  passed the whole suite; the only coverage was the no-makers case. It is the check that stops the
  decision loop filling an order that costs more to source than the swapper pays.
- **Four order shapes the reactor reverts on were accepted**: a decay window that does not advance
  (`EndTimeBeforeStartTime`), an input that falls or an output that rises (`IncorrectAmounts`), and
  a cosignature whose recovery byte is 0/1 rather than 27/28 — `ecrecover` yields the zero address
  and the reactor rejects it, but `Signature::from_raw` normalises it and we accepted the order.
- **A feed whose first poll never succeeded reported healthy forever.** Readiness now measures from
  the first attempt when nothing has ever succeeded.
- **The native-currency admission rule had no effective test** — the three that appeared to cover it
  were all satisfied by the token allow-list instead, since the zero address is never admitted.
- **Dutch decay rounded the wrong quantity.** `AmountCurve::amount_at` rounded the resolved amount;
  `DutchDecayLib.decay` rounds the *distance travelled* and then adds or subtracts it — `mulDivDown`
  when falling, `mulDivUp` when rising, so both directions move against the filler. One wei out
  otherwise, and a wei short is a fill the reactor refuses. The `u128` differential oracle mirrors
  both branches, with its vectors pinned from the `contracts/lib/UniswapX` submodule the fill E2E
  source-deploys.
- **Only the first output leg was sourced.** An order paying the swapper and an interface fee
  recipient in the same token had the rest discovered on chain — after every maker leg had been
  bought and paid for in gas.
- **The exclusivity toll rounded on the aggregate.** `ExclusivityLib` iterates the resolved outputs
  and applies `mulDivUp` to each, so the round-up happens once per leg; rounding the sum instead
  under-sources by up to one wei per extra leg. Same revert, same point in the fill.
- **The order-feed config shipped inert.** Every new key sat below the last `[[price_symbols]]`
  header, and TOML scopes bare keys to the table above them — so the keys were parsed as fields of
  that entry and discarded, `orders_api_url` read as absent, and the feed, ingest and decision loop
  never started with nothing logged. `PriceSymbol` now refuses unknown fields, so the same mistake
  fails the parse instead.
- **The no-op telemetry macros discarded their arguments**, making a variable that is only ever
  logged read as unused — so the crate was warning-clean only under whichever feature set enabled
  `tracing`, and any per-crate `-D warnings` job failed while the workspace one passed.
- **Trade surplus uses the input token's decimals and USD price.** Exact-out routing records expected
  retained-input profit, net of estimated gas. Formatting it as output token units made DAI→USDC
  profit appear a trillion times too large; the amount and valuation now use the actual denomination.
- **Price impact no longer falls as the trade grows.** The near-zero baseline was probed at a
  millionth of the trade size, which quotes only a handful of whole base units — a rate wrong
  enough that the real one looked better than it, and the size of that gap was reported as impact
  (24.98% on a 2.5 DAI trade against a $500k pool; a saturated `u64::MAX` at the limit). The probe
  now widens until its output can be divided meaningfully.
- **A tag is a label, not a key** — stablecoin classification matched `"stables"` case-sensitively,
  so a catalog that cased a tag for people to read would silently stop classifying it.
- **A configured stablecoin peg reports a 0% daily move**, not an absent one: a token held at par by
  configuration has not moved, which is different from having no reading.
- **Depth bisection stops on a relative tolerance** — converging to the last wei cost ~40 further
  rounds of curve math for precision no caller can observe.

_Next: Aqua swap-vm v1.0.2 parity, then contract hardening._

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
