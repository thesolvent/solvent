# Arbitrage-by-design — maker LVR recapture (Tier 0) — design

> **Status:** design of record, awaiting review. **Target:** ETHOnline 2026 (submit Sept 13).
> Scope: **Tier 0, same-chain** — return the arbitrage that same-direction flow leaks to MEV bots
> back to the maker whose pool it drained, by internalizing the reverse (rebalancing) trade. The
> public counter-intent fallback (Tier 1) and cross-chain are designed here at the boundary but
> **deferred**. Backend phases B0–B4 (watcher → routing → ledger → execution) are prerequisites.

> **Scope decision (2026-09-06) — internal-only.** After Tier 0 shipped (with actual-vs-expected
> reconciliation and a realized-spread cap, §5.2/§14), Tier 1 (public counter-intent auction) and its
> ledger-race concurrency work (§6.5) were **taken out of scope**, not merely deferred: the novelty and
> the zero-inventory thesis live in the internal path, that race exists only for Tier 1's waiting
> orders, and best-effort recapture is an accepted, disclosed limit (§12). The remaining Tier-0 items —
> the in-binary payout worker + its config (§6.2/§8) and the oracle-staleness gate (§8/§9) — are
> deferred as **blocked on infrastructure that does not yet exist** (the app composition root; a
> caching/timestamped price oracle). The recapture mechanism itself is complete and proven on-chain.

---

## 1. Why this exists — the leak

Solvent fills a swap by borrowing a maker's Aqua/SwapVM pool for the length of one atomic tx,
delivering to the swapper and keeping the spread ([`route`](../../../crates/core/src/routing/service.rs),
[`UniswapXAquaFiller`](../../../contracts/src/UniswapXAquaFiller.sol)). It holds no inventory.

When flow is **one-directional**, that borrowing systematically mis-prices the maker's pool. Four
swappers selling `ETH→USDC` each push ETH into one maker's pool and pull USDC out. After the
fourth, the pool is ETH-heavy and USDC-light, so **inside the pool ETH is now under-priced** (too
much of it relative to the constant-product curve's idea of fair).

That gap is free money for an outsider: an MEV searcher buys the cheap ETH from the pool
(`USDC→ETH`), sells it at the real market price, and keeps the difference. The **maker eats the
loss** — this is loss-versus-rebalancing (LVR). Today that value leaves the system entirely to a
searcher who supplied nothing but a bot.

**Goal:** keep that rebalancing trade *inside Solvent*, so the recaptured value is split between
the maker (who is made mostly whole) and the protocol (a new fee earned from the arbitrage),
instead of leaking to a stranger.

---

## 2. The idea, and the one economic fact that shapes it

**Idea.** After a maker's pool is left cheap by forward flow, the *reverse* swap that fixes it
should be routed through Solvent, and the arbitrage profit that reverse swap yields should be
shared back to that same maker (recapture), with the protocol keeping a cut.

**The fact that shapes the whole design.** You cannot recapture LVR by having the maker trade
against their *own* pool — that only moves value from the maker's pool to the maker's wallet, net
zero. Real recapture requires an **external counterparty who pays fair value**. On a public,
non-gated pool the only such counterparty that arrives on its own is **organic reverse demand**:
a real `USDC→ETH` swapper. So Tier 0 recapture is *internalization* — matching the imbalance
against the next opposite intent — not extraction from the pool.

**Why this stays public and zero-friction.** We do **not** gate maker pools (a searcher may still
arb directly; we simply try to be the one who does it, on the maker's behalf, when organic reverse
flow lets us). Recapture is therefore **best-effort**: it fires when reverse flow naturally routes
into the cheap pool. That is an accepted limitation (see §12), and the price of requiring nothing
of the passive maker.

---

## 3. Prior art (and how Tier 0 relates)

| System | How it recaptures | Enabler | We borrow / differ |
|---|---|---|---|
| **CoW AMM** ([cow.fi/cow-amm](https://cow.fi/cow-amm)) | Solvers compete to rebalance a *protected* pool; the one returning the pool the most surplus wins | Pool only trades via the auction (gated) | We take the *surplus-to-LP* goal, but **without gating** — so no on-pool auction (that is Tier 1) |
| **MEV-Share / MEV Blocker OFA** ([docs.mevblocker.io](https://docs.mevblocker.io/concepts/order-flow-auction)) | Searchers bid to backrun; ~90% of the value is rebated to the order's origin | Private order flow | We adopt the **generous fixed rebate share** norm (auction-less), paid to the maker |
| **Angstrom / Auction-Managed Liquidity** ([Arrakis](https://arrakis.finance/blog/the-amm-renaissance-how-mev-auctions-and-dynamic-fees-prevent-lvr)) | Auction the right to arb; proceeds → LPs | App-specific sequencing | Same family as Tier 1; out of scope here |
| **1inch Fusion** | Dutch-decays the *resolver fee* from max→0 | Solver auction | Informs the Tier 1 counter-intent shape (fee decay), deferred |

**Honest novelty.** Not "MEV recapture" (CoW/Angstrom own that) and not "internalization" (pro
solvers net flow already). What is specific here: recapture delivered as a **post-settlement
accounting layer over a zero-inventory intent resolver** — no gating, **no new on-chain contract**,
returning LVR to a *passive* Aqua maker who did nothing but supply liquidity. It rides the existing
filler and the routing you already have.

---

## 4. Scope — the recapture waterfall, and what Tier 0 is

The full idea is a three-tier waterfall. **This document builds Tier 0 only.**

- **Tier 0 — Internalize (this doc).** Route the reverse trade into the imbalanced pool (already
  the router's natural behavior — see §6.1) and rebate the maker a share of the arb. Real
  recapture; best-effort (needs reverse flow).
- **Tier 1 — Public counter-intent (deferred).** When no reverse flow arrives in the imbalance's
  window, originate a public Dutch order so an external filler rebalances the pool. Restores pool
  health with an auction-set split. Requires solving "originate an order with no user/inventory."
- **Cross-chain (deferred).** Same idea across the source/destination split of §10 of `SPEC.md`.

---

## 5. Mechanism — honest, oracle-referenced, stateless

Recapture is computed **once per confirmed fill**, from the fill's own legs and a fair-value
reference, with **no cross-time imbalance ledger**.

### 5.1 Per-leg LVR (in a USD numeraire, via the existing oracle)

For each maker leg of a confirmed fill — a [`RouteLeg`](../../../crates/core/src/primitives/routing/mod.rs)
`{ maker, token_in, token_out, amount_in, amount_out }`, where the maker **received** `amount_in`
of `token_in` and **gave** `amount_out` of `token_out`:

```
value_given  = amount_out × price(token_out)      // USD, from PriceOracle
value_recv   = amount_in  × price(token_in)        // USD
leg_LVR      = max(0, value_given − value_recv)     // > 0 only when the maker sold cheap
```

`leg_LVR` is positive **exactly on rebalancing legs** (maker gives more value than it gets).
Forward/imbalancing legs — where the maker *buys* cheap — compute `0`, so credits land only on
genuine rebalancing. No pool reserves, no history, needed.

### 5.2 The split

```
rebate_usd   = maker_share × leg_LVR                 // USD value returned to the maker
Σ rebate_usd  ≤  α × fill_spread_usd                 // cap: never pay out more than the fill earned
rebate        = rebate_usd / price(token_in)          // paid in token_in — the side the maker is short
credit        = { maker, token: token_in, amount: rebate }
protocol_take = fill_spread − Σ rebate                // a slice of a fat spread ⇒ > a normal fill's
```

where `fill_spread` is the fill's resolver spread — the `RoutePlan`'s
[`expected_profit`](../../../crates/core/src/primitives/routing/mod.rs) in the MVP (§12 covers
using `SettlementReader` actuals instead).

- `maker_share` ≈ **80%** (config; the MEV-Blocker-flavored generous rebate).
- The **rebate token is `token_in`** — the token the maker received in the rebalancing trade, i.e.
  the side its pool was drained of by the forward flow; topping it up most directly compensates the
  maker. In the §5.3 example that is USDC.
- `α` caps total rebates at a fraction of what the fill actually earned, so **the resolver never
  pays out more than it made** — recapture is bounded by realized spread.
- The protocol's cut is a *slice of a fat spread*, so on a recapture fill it is **strictly larger**
  than the thin spread of an ordinary fill: the protocol earns from the arbitrage, by design.

### 5.3 Worked example (mid = 1 ETH = 3000 USDC)

Normal forward fill (pool still balanced) — the resolver only ever *keeps* a spread; the router
pays the maker the pool's (fair) price:

```
   ALICE            SOLVENT (filler + router)         MAYA'S POOL
 1 ETH ───────────────►│── 1 ETH ───────────────────►│ ETH in
                       │◄────────────── 3000 USDC ────│ USDC out (fair)
 ◄──── 2990 USDC ──────│ keeps 10 USDC (spread)
```

Repeat 4× → Maya's pool is ETH-heavy → it will now sell 1 ETH for only **2700**. A reverse buyer
arrives; recapture fires:

```
   BOB              SOLVENT (filler + router)     MAYA'S POOL (cheap ETH)
 3000 USDC ─────────►│── 2700 USDC ───────────────►│ USDC in
                     │◄──────────────── 1 ETH ─────│ ETH out (rebalances)
 ◄──── 1 ETH ────────│ earns 300 spread
                     │── rebate 240 USDC ──────────► MAYA   ★ recapture
                     │ keeps 60 USDC                          (paid off-chain, batched — §6)
```

`leg_LVR = 1×3000 − 2700 = 300`. With `maker_share = 0.8`, `α = 1.0`:

| Party | Normal fill | Recapture fill |
|---|---|---|
| Swapper | pays ~market | pays ~market |
| Maker | fair price, no loss | 2700 pool **+ 240 rebate = 2940** (recovers 240 of a 300 loss) |
| **Protocol** | ~10 spread | **60** — earns from the arb |
| Pool | (unchanged) | **rebalanced** |

The 300 is money that today leaks entirely to a bot; both maker and protocol end ahead of the
status quo.

---

## 6. Architecture

Hexagonal, matching the existing slices (`ingest`, `registry`, `routing`, `ledger`, `execution`).
Recapture is **additive**: a pure primitive + a slice + two ports + an app worker. **No routing
rewrite, no new on-chain contract, no direct Aqua interaction.**

### 6.1 What already does the work (unchanged)

- [`select`](../../../crates/core/src/routing/candidates.rs) ranks pools by *most output for the
  trade*; [`waterfill`](../../../crates/core/src/routing/waterfill.rs) splits at the marginal
  optimum. A cheap (imbalanced) pool offers the most ETH per USDC, so it **already ranks first and
  receives the reverse flow**. Tier 0 adds **no** imbalance bias to routing (decision §13).
- [`SettlementReader.settled`](../../../crates/core/src/execution/service.rs) already yields the
  confirmed fill's actual per-source amounts.
- The `PriceOracle` port ([`deps/routing/price_oracle.rs`](../../../crates/core/src/deps/routing/price_oracle.rs))
  already prices tokens in USD — **reused** as the fair-value reference; no new port.

### 6.2 New units

| Unit | Path | Responsibility |
|---|---|---|
| `RecaptureCredit`, `RecapturePolicy`, `recapture_split()` | `crates/core/src/primitives/recapture/` | **Pure** value types + the §5 computation. Zero I/O; the whole formula is here and unit-tested. |
| `RecaptureService` | `crates/core/src/recapture/service.rs` | On a confirmed fill: fetch prices via `PriceOracle`, call `recapture_split`, `accrue` credits. |
| `RecaptureStore` (port) | `crates/core/src/deps/recapture/store.rs` | `accrue`, `outstanding`, `mark_settled`; own `RecaptureStoreError`. Durable owed-per-maker balances. |
| `RebatePayer` (port) | `crates/core/src/deps/recapture/payer.rs` | `pay(batch) -> PayoutHandle`; own `RebatePayerError`. Submits the batched rebate transfer. |
| sqlite `RecaptureStore` + `RebatePayer` adapters | `crates/adapters/src/recapture/` | Persistence (a `recapture_credits` table) and the on-chain payout tx builder. |
| Payout worker | `crates/app/src/` | Periodically drains `outstanding`, batches, calls `RebatePayer`, `mark_settled`. |

### 6.3 Wiring

The app's decision loop already holds each intent's [`RoutePlan`](../../../crates/core/src/primitives/routing/mod.rs)
(its `legs` and `expected_profit`) when it builds the fill. On confirmation —
[`reconcile`](../../../crates/core/src/execution/service.rs) surfaces the set of newly-confirmed
intents (a small change: it returns the confirmed `(intent, actuals)` rather than `()`) — the app
hands that intent's `legs` + `expected_profit` to `RecaptureService`. Recapture never sits on the
fill hot path; it runs after settlement.

### 6.4 On-chain footprint: zero new contract

The resolver's spread already accrues in the filler and is swept to the operator treasury. Rebates
are paid as **operator-signed ERC-20 transfers from that treasury to maker addresses**, batched by
the payout worker. Nothing is pushed to Aqua; the "one thin contract" thesis is untouched.

### 6.5 Concurrency & ledger invariants

**Tier 0 cannot hit the "counter-trade mid-flight" race.** Recapture runs *after* `post` (§6.3),
creates **no reservation holds**, and only reads the settled fill — so it cannot conflict with a
concurrent trade. Its lone requirement is per-intent **idempotency** (no double-credit if
`reconcile` re-runs), handled the way execution already swallows a repeated `post` via
`LedgerError::WrongState`. Recapture stays purely observational, so the Tier 1 integration drops in
cleanly.

**The race is a Tier 1 (auction) concern** — a posted counter-intent *waiting to fill* while a
counter-trade rebalances the same maker. The existing single-writer ledger already supplies the
guardrails; Tier 1 must use them:

1. **Reserve through the ledger, not a free promise.** A Tier 1 order takes a `Pending` reservation
   on the maker's capacity; a concurrent draw then competes for the *same* capacity and
   `can_reserve` admits **exactly one** (proven by `alice_race_admits_exactly_one` /
   `shared_wallet_admits_exactly_one`). No double-commit, by construction.
2. **Validity tied to the imbalance.** TTL + [`sweep_expired`](../../../crates/core/src/ledger/service.rs)
   releases an unfilled hold; the reconciler `void`s an open order once the maker's pool is observed
   rebalanced (its "cheap pool" premise is gone).
3. **Fail-safe price bound.** The order carries a min-out / max-in; if the pool moved, the fill
   reverts or the sim gate rejects it rather than executing at a stale price.
4. **Single-writer serialization.** Every transition (reserve/void/expire/post) runs under the one
   durable-first `LedgerService` mutex, so "counter-trade meanwhile" is serialized against "auction
   fill," never interleaved into an inconsistent state.
5. **On-chain backstop.** Aqua reverts an over-draw regardless (SPEC §8) — worst case is wasted gas,
   never a double-spend.

**Testing (built in the Tier 1 phase).** Extend the ledger property test
[`conserves_and_never_over_promises`](../../../crates/core/src/primitives/ledger/engine.rs) so its
command stream interleaves an auction order's reserve/void/expire with fills, plus targeted races:
(a) auction order pending + counter-trade draws the same maker → exactly one commits; (b) auction
order pending + pool rebalanced → order voided, not filled; (c) TTL expiry restores the hold.

---

## 7. Data flow (end to end)

```
forward ETH→USDC fills ──► maker M's pool goes ETH-heavy / cheap  (no recapture: leg_LVR = 0)
        │
        ▼
reverse USDC→ETH intent arrives
        │  routing (unchanged) picks M — it offers the most ETH
        ▼
fill confirms ──► app hands M's leg(s) + realized spread + oracle prices ──► RecaptureService
        │                                                                      │
        │                                              recapture_split → RecaptureCredit{M, token, amt}
        ▼                                                                      ▼
   pool rebalanced                                                     RecaptureStore.accrue
                                                                               │
                                              payout worker (cadence) ── RebatePayer.pay(batch) ──► M's address
```

---

## 8. Configuration

Per-chain / per-operator `RecapturePolicy` (in [`primitives/shared/config.rs`](../../../crates/core/src/primitives/shared/config.rs)
alongside the existing config):

| Knob | Meaning | Starting value |
|---|---|---|
| `maker_share_bps` | maker's cut of `leg_LVR` | 8000 (80%) |
| `spread_cap_bps` (`α`) | max rebate as a fraction of realized spread | 10000 (100%) |
| `min_credit` | dust floor; skip credits below it (gas > value) | per-token, small |
| `payout_interval` / `payout_threshold` | batch cadence and/or minimum owed to trigger a payout | e.g. every N blocks or ≥ threshold |
| `oracle_max_staleness` | reject recapture if the fair-value price is older than this (fail-closed → 0 credit) | conservative |

---

## 9. Errors & observability

- Every public fallible API returns `SolventError`; `RecaptureStoreError` / `RebatePayerError` map
  in via `From` (per-port enums, one file each).
- Recapture is **fail-open for the fill, fail-closed for the credit**: a missing/stale oracle price
  or a store error **never** reverses or blocks a settled fill — it logs and yields **zero credit**
  for that leg. A fill must never fail because recapture could not be computed.
- Instrument via the `obs` shim, correlated by `IntentId` (one span per intent, consistent with the
  rest of the engine). Levels: WARN on skipped credit (stale oracle, store error) / failed payout;
  INFO once per intent on accrued recapture; DEBUG for the per-leg split. No secrets on any path.

---

## 10. Testing

Regression-worthy only (per house rules): the computation and the end-to-end recapture, not glue.

- **Unit — `recapture_split`** (pure, in-memory): `leg_LVR` sign (positive on rebalancing, zero on
  imbalancing legs); the `α × spread` cap binds; `maker_share` split arithmetic; exact-in and
  exact-out symmetry; multi-leg attribution; dust floor.
- **Headline integration — the recapture loop** (mirrors the forced-contention test's style): seed
  maker M → run N forward `ETH→USDC` fills to imbalance M → submit one reverse `USDC→ETH` intent →
  assert (a) routing sends it to M, (b) M's realized LVR drops from `L` to `L − rebate`, (c) the
  resolver still nets ≥ 0, (d) a credit is accrued and the payout worker settles it. Hermetic
  (in-memory fakes for the store/payer/oracle; the existing anvil loop for the on-chain leg).
- Each new test ships with its exact single-test re-run command.

**Build order (verify-first):** the headline integration test is built **first**, as a thin walking
skeleton through the real watcher → route → reserve → execute → settle → recapture loop, to prove the
MVP end-to-end before the policy/payout are fully fleshed out.

---

## 11. YAGNI — deliberate non-goals

Not built, and why:

- **No cross-time imbalance ledger** — the per-fill oracle-referenced `leg_LVR` (§5.1) makes it
  unnecessary.
- **No routing change** — the natural cheap = best-priced preference already internalizes.
- **No Tier 1 counter-intent origination** — deferred; needs the "originate an order with no
  user/inventory" problem solved.
- **No separate protocol-vs-resolver accounting** — in the MVP the resolver *is* the protocol, so
  the protocol's take is simply retained spread; no fee-routing type until a real second consumer.
- **No donate-into-pool payout** — would make the filler call Aqua directly; rejected for the
  thin-contract invariant.
- **No `ErrorKind`/classification on the new errors** until a caller branches on it.

---

## 12. Honest limits & risks

- **Best-effort recapture.** Fires only when reverse flow routes into the cheap pool within its
  useful window. No reverse flow ⇒ no recapture (the pool is still arbable by an outside searcher,
  as today). Guaranteed rebalancing is Tier 1.
- **Bounded by realized spread.** `α × realized_spread` caps the rebate; a thin fill recaptures
  little even if the pool was very cheap.
- **Oracle dependence.** Fair value is only as good as the `PriceOracle`; staleness ⇒ fail-closed
  (zero credit), never a wrong overpayment.
- **Payout trust window.** Rebates are batched, so the maker briefly trusts the operator to pay.
  Acceptable same-chain; makers can watch accrual. (Atomic payout is a Tier-2-era option.)
- **Expected vs actual leg amounts.** The MVP computes from the `RoutePlan`'s expected leg amounts
  on a confirmed fill; partial-fill drift is a documented follow-up (reconcile against
  `SettlementReader` actuals).

---

## 13. Decisions locked in the design brainstorm

| Decision | Choice | Why |
|---|---|---|
| Objective | Recapture LVR **for makers** | Attracts passive liquidity; the honest value-add |
| Exclusivity | **Public, no gating** | Zero maker friction; recapture funded by internalization instead |
| Primary path | **Internalize-first** (Tier 0), public fallback deferred | Cleanest, most Solvent-native, strict zero-inventory |
| Scope | **Same-chain**, Tier 0 only | Smallest honest thesis-proof; fits the hackathon |
| Sizing | **LVR-based**, `min(share×LVR, α×spread)`, share ~80% | Targets genuine mispricing; auction-set once Tier 1 lands |
| Payout | **Off-chain accrual + batched operator transfers** | Keeps the one-thin-contract thesis; logic stays in Rust |
| Routing | **No explicit imbalance bias** | The natural cheap = best preference already steers reverse flow |
| Fee model | Build the **maker/protocol split** (measured, config); defer a general fee framework | The protocol's arb-take is non-extractive revenue (earned from arb, not charged to users); a broader fee model has no consumer yet (YAGNI) |

---

## 14. Open questions / future

- **Tier 1 counter-intent origination** — *out of scope (internal-only, see the scope decision above).*
  How Solvent would post a public Dutch rebalancing order with no user and no inventory (synthetic
  UniswapX order vs. a Solvent-native open order to a filler network), so recapture no longer depends
  on organic reverse flow.
- **Auction-set split** — *out of scope (rides on Tier 1).* Once Tier 1 exists, the maker/protocol
  share becomes competition-set (CoW/Fusion norm) rather than a fixed 80/20.
- **Actual-vs-expected reconciliation** — *done.* Credit is valued from the fill's actual legs
  (`SettledLegsReader`), and the α cap uses the fill's realized spread (`realized_spread`), rather than
  `RoutePlan` expectations.
- **Cross-chain recapture** — *future.* Reverse flow and pool live on the destination chain; interacts
  with the netting/rebalance layer (L4) of `SPEC.md` §10.
