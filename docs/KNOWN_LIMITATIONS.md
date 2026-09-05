# Known limitations & deferred work

Correctness gaps found by adversarial corner-case review that are **intentionally deferred** to a
later phase (each is documented at its code site too). None is an over-commitment or fund-loss risk:
every one degrades safely (under-report / miss a fill / self-heal on restart). Fixed findings are not
listed here — they live in the code and its tests.

The review method, for reuse: per component, trace the code against ① a tombstone/inactive/terminal
state read as if live, ② idempotency/replay safety, ③ external state changing under us, ④ numeric
edges, ⑤ ordering / concurrency / crash-consistency.

---

## L1 — Late-`Shipped` balance loss in the live snapshot
**Component:** registry — `crates/core/src/registry/sync.rs` × `crates/core/src/primitives/registry/snapshot.rs`
**Class:** ⑤ ordering (cross-cycle)

`Snapshot::apply` drops a `Pushed`/`Pulled` to a not-yet-registered strategy (correct per event — you
cannot push to a strategy that has not shipped). The sync applies each newly-inserted event once, then
marks it seen. So if a block's `Shipped` (log 0) is delivered in a **later** sync cycle than that
block's `Pushed`es (RPC partial-block delivery dropping the lowest log index — the same assumption the
`applies_late_arriving_event_below_the_tip` test encodes), those `Pushed`es are applied-and-dropped,
stored, marked seen, and **never re-applied** when the `Shipped` finally lands. The live snapshot then
under-reports that strategy's virtual balance.

- **Blast radius:** the ledger's strategy-virtual budget reads low → it **under-promises** (misses
  fills). Never an over-commit. **Self-heals** on `recover()` / restart (recovery folds the durable
  log in order).
- **Within-cycle** out-of-order delivery is already handled — `sync_once` sorts the inserted batch by
  cursor before folding (test `folds_inserted_in_cursor_order_regardless_of_source_order`). This item
  is only the **cross-cycle** case.
- **Disposition:** repaired by the planned **periodic reconcile (Task M — multicall balance
  reconciliation)**, which re-reads on-chain balances and corrects snapshot drift. A targeted fix
  (re-fold a strategy's stored events when its `Shipped` registers late) is possible but more code for
  a rare, safe-direction, self-healing case.

## L4 — Reorg replacement logs are not yet distinguished
**Component:** registry — `crates/core/src/registry/sync.rs`
**Class:** ⑤ crash/chain-consistency

The sync's in-memory `seen` cache keys on `(block, log_index)`, while the store's unique key is
`(chain, block, block_hash, log_index)`. A reorg's replacement log (same position, new block hash)
would be filtered by the `seen` cache as "already seen" and never reach the store or snapshot live.

- **Blast radius:** stale snapshot across a reorg until restart; the block hash provenance needed to
  detect it is already stored.
- **Disposition:** **Task R (reorg handling)** — detect a stored-vs-fetched block-hash mismatch, prune
  the non-canonical rows, and rebuild the in-memory snapshot. The `seen` cache will then key on (or be
  bypassed by) the block hash.

## L5 — `price()` does not filter inactive strategies
**Component:** registry — `crates/core/src/registry/pricing.rs`
**Class:** ① inactive-as-live

`price()` prices whatever `MakerStrategy` it is handed, including a docked tombstone (which keeps its
old balance). Correctness relies on callers routing only active strategies via
`Snapshot::active_strategies_for_pair`, which they do. (The ledger's `AlloyBudgetSource` was the one
path that read a strategy balance without this filter — now fixed to check `active`.)

- **Blast radius:** none today (no caller prices an inactive strategy).
- **Disposition:** latent inconsistency; documented at the call site. Revisit if a non-routing caller
  ever needs to price directly.

## L6 — Registry ↔ ledger startup ordering & mid-flight virtual drift
**Component:** integration — `crates/core/src/ledger/service.rs` (`recover`) + composition root
**Class:** ③ external-state / ordering

1. **Startup ordering:** `LedgerService::recover()` reads budgets from the current sources, so the
   **registry must be synced/recovered before the ledger recovers**, or a strategy virtual reads zero
   until its next reserve refreshes it. This is a composition-root sequencing requirement (documented
   at `recover`).
2. **Mid-flight drift:** a strategy's virtual can drop between reserve and settlement (another taker
   swaps it down) — the same class as the *tested* wallet-budget drop. Caught at reserve time by the
   JIT firm confirm; a drop *after* reserve is caught by reconcile, with the on-chain `pull` revert as
   the final backstop.

- **Disposition:** (1) enforce in the composition root when the app binary is wired; (2) reconcile
  (Task M) + the on-chain revert already cover the after-reserve drop.

## L7 — Pegged legs under-fill slightly at the marginal-price optimum
`fill_to_limit_numerical` (the partial fill for curves with no closed-form inverse, i.e. Pegged)
finds the fill by bisecting a **finite-difference** marginal with a step of `feasible_bound / 1e6`.
The secant lies below the true tangent on a concave curve, so the bisection stops where the *secant*
reaches λ — a touch before the true marginal does — and the pegged leg under-fills. The KKT oracle
measured this at up to ~18 % marginal deviation on a pegged leg vs the closed-form XYC/Concentrate legs
(which land on λ exactly); the output loss is far smaller (the gap is integrated over a small fill
delta). This is why the KKT residual check excludes pegged legs (they're still covered by the
unused-leg violation check). → refine the fill with a Newton/secant step after the bisection, or a
step local to the fill point, to land the marginal on λ tightly. Quality, not correctness.

## L8 — Sparsity heuristic is near-optimal only while gas is a small fraction of leg output
`solve_sparse` prunes legs by dropping the smallest-output one while that improves the resolver's
net take — a greedy hill-climb, not the true `2^K` gas-aware optimum. The subset-optimality study
measured its regret against the exhaustive optimum across gas levels: **≤37 bps at realistic gas
(~2 % of a leg's output), but ~5 % at gas = 10 % of output and up to 37 % at gas = 50 %**. So on
gas-heavy trades — small trades where per-leg gas rivals a leg's output, which are barely economical
anyway — the heuristic can leave a few percent on the table. → if such trades ever matter, swap
drop-by-smallest for drop-by-marginal-contribution or forward-selection (both closer to the `2^K`
optimum). Quality, not correctness; the split is always valid and reservable.

## L9 — The out@size funnel ranking ignores capacity
`select` keeps the top-K candidates ranked by estimated net output at the trade size
(`net_quote_exact_in(amount)`). That favours good-price pools and is **blind to input capacity**, so
when a pair has more than K pools the top-K can lack the combined capacity to absorb the trade — a
high-capacity, moderately-priced pool the optimum leans on gets ranked just out of the funnel. The
funnel-decomposition study confirmed it at forced-small K (e.g. seed 22: the top-4-by-out@size hold
1702 of a 2464 input, so they can't fill a trade the optimum's support-4 fills easily). **Not a
current issue** — the shipped K=64 is far above any pair's pool count today, so the funnel never
drops — but as registries grow past ~64 pools/pair it will. → add a capacity signal to the ranking,
or a capacity floor that keeps adding pools until the top-K can absorb the trade. Quality, at scale.

---

# Performance — refactor before production

MVP-simple choices that are correct but do unwanted work / hold heavy state. None is on the 500 ms
quote path (that path is in-memory / lock-free reads); all are on the slower write/sync routes.

## P1 — Full snapshot clone every sync cycle
`RegistrySync::sync_once` does `(*self.snapshot.load()).clone()` then re-publishes — an O(strategies)
deep copy of the whole registry on **every** cycle that inserts an event. Fine at MVP scale, wasteful
as the registry grows. → apply the fold to a `Arc::make_mut` / persistent structure, or batch
publishes, or a copy-on-write snapshot with structural sharing.

## P2 — Per-cycle sort of the inserted batch
`sync_once` sorts the newly-inserted events by cursor each cycle for a robust fold order. Cheap
(batches are small) but strictly redundant when the source already returns sorted logs. → drop it once
the `ChainSource` contract guarantees cursor order, or sort only when out-of-order is detected.

## P3 — `available_snapshot()` rebuilt on every ledger command
`LedgerService::publish` rebuilds the entire `AccountKey → available` map (`available_snapshot`) and
swaps it into `ArcSwap` after every reserve/post/void. O(accounts) per command. → publish incremental
deltas, or shard the snapshot by maker/token.

## P4 — Writer lock held across the durable write
`LedgerService` holds its `tokio::Mutex` across the store `.await`, serializing all writers on the
(local, fast) SQLite write. Correct (single-writer by design) and fine for SQLite, but it caps write
throughput. → group-commit batching, or the command-actor / Disruptor model, when write volume demands.

## P5 — JIT budget confirm is one multicall per wallet account, not batched across sources
`AlloyBudgetSource` issues one Multicall3 round-trip per `WalletBudget` account. A multi-source reserve
therefore makes N round-trips. → batch the whole reserve's confirm into a single multicall (design
calls for "1 batched JIT confirm"). Also: the event-sourced zero-RPC budget cache replaces this on the
quote path entirely (a later phase).

---

# Deferred routing test coverage

The routing correctness + quality suite (Tiers 0–3, `docs/plans/backend/routing-experiments-plan.md`)
was completed as a value-focused subset: Tier 0 invariants, Tier 1 oracles (KKT/round-trip/monotonicity),
Tier 2 quality (sparsity #31, funnel #32), and one Tier-3 axis (extreme-scale robustness). The remaining
20-axis-matrix studies were deferred as lower-yield — each either folds into an oracle already run, or
re-confirms a known finding, or needs calibrated market data. Pick up if a specific concern arises:

- **Adversarial book search** (CMA-ES / hill-climb maximising regret@K) — the solver adversary is covered
  by the KKT optimality oracle, the funnel adversary by L9; would re-derive both.
- **Realism replay** (calibrated depths/fees + log-normal sizes → bps given up per unit volume) — dominated
  by #31's sparsity regret (≤37 bps) with the funnel inactive at K=64; needs real market data.
- **Warm-start economics** (cold vs same/stale/adversarial λ — iteration counts), **tol/iter Pareto**
  (regret vs p99 as `MAX_ITERS`/tol move), **two-stage funnel at large n** (spot prefilter n→256→K),
  **temporal** (staleness/churn/depletion), **interaction discovery** (variance decomposition over an LHS
  sample) — tuning/characterisation studies, valuable once there is production load to calibrate against.
- **Quote-call → latency predictor** — the counter now exists (`--features quote-metrics`); building the
  full per-curve unit-cost model is deferred until the `BigRational`→fixed-point decision is on the table.

# Deferred ingest work (B4)

- **`replay` order feed** — deferred to **B7 (backtest)**, its only real consumer. Building it in B4
  would only support a self-referential "replay ≡ self_hosted" test and would fix a serde archive
  format before the backtest defines what it needs. The design's "self_hosted ≡ replay identical
  stream" done-when moves to B7.
- **`hosted` poll feed** (Uniswap Orders API, 6 rps, 429 backoff, re-poll backfill) — deferred to the
  mainnet target; v1 depends on no hosted service. Needs its own E2E/contract test against the live
  API (or a faithful mock) when built.
- **RFQ `QuoteServer` (Mode 2)** — deferred until the Ledger soft-hold exists and there is a real
  deployment to contend on; a local rig can't demonstrate real RFQ competition.
- **Cosigner override-bounds pre-filter** — the normalizer rejects a cosigner `outputAmounts` whose
  length mismatches the outputs (an always-reverting order), but does not yet reject out-of-bounds
  override *values* (`inputAmount > baseInput.startAmount`, `outputAmounts[i] < baseOutput.startAmount`),
  which the reactor also reverts. Our own builder never emits these; a pre-filter earns its place once
  the `hosted` feed ingests third-party orders.
- **Multi-output / exact-output (input-decaying) coverage** — `OrderSpec`/`SelfHostedFeed` only build
  single-output, static-input orders, so the tests don't exercise multi-output or the ceil/input-decay
  path end-to-end. The curve math is proven direction-agnostic in `curve.rs`; add order shapes when a
  protocol/order needs them.

# Deferred execution work (B5)

- **Post-finality reorg detection** — the `ExecutionService::on_reorg` → `ledger.void_reorg` coupling
  is wired and tested, but the *trigger* (detecting that an already-confirmed fill un-mined deeper than
  the confirmation depth) is deferred to **B6 (reconcile)**, watching the canonical chain. walletkit
  absorbs sub-confirmation reorgs itself, so the normal path never calls it.
- **In-flight crash recovery** — the service's in-flight map (intent → handle + reservation) is
  in-memory, so a crash mid-fill loses the tracking. walletkit's durable store still holds the tx and
  the ledger still holds the open reservation; rebuilding the map from those on restart is **B6**.
- **Batch fills** — `fillBatch` + fate-compatible grouping + all-post-or-all-void reservation sets are
  deferred: the router emits one `RoutePlan` per intent, so batching across intents has no consumer
  yet. The single-fill loop is the full production path.
- **revm fork-sim gate** — v1 simulates via walletkit `dry_run` (eth_call at head), which the P1
  filler's on-chain guards make sufficient for the reject decision. A richer revm fork-sim (state
  overrides, exact profit net of gas) is a second `SimGate` adapter behind the same port.
- **Per-fill gas ceiling** — the RBF bump loop is bounded by a static wallet-level `gas_ceiling`, not
  by *this fill's* expected profit; a per-fill-tight ceiling needs a per-send gas envelope in walletkit.
- **`Dropped` / deep-reorg E2E** — the reservation `void` on a dropped fill and the `void_reorg`
  coupling are unit-tested with fakes; forcing a deterministic on-chain nonce-steal or reorg belongs to
  walletkit's own localnet harness, so the anvil E2E covers the happy + sim-reject paths.
- **Finality-stall confirmation freeze** — walletkit anchors confirmation on the chain's `finalized`
  tag when the RPC exposes one (falling back to a depth count only when it does not). If a chain stops
  finalizing, confirmations freeze (fills stay tentative `Mined`) rather than settling on depth alone —
  correct (nothing is truly final during a stall), but worth operational awareness. Anvil never
  advances the tag by default, so the E2E runs the node with `--slots-in-an-epoch 1`.
