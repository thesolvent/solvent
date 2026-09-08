# Tier 0 recapture — phase plan (locked)

> Design of record: [`2026-09-05-arbitrage-recapture-design.md`](2026-09-05-arbitrage-recapture-design.md).
> Same-chain maker-LVR recapture MVP. **Branch off `main`** at the phase-commit gate; implement
> uncommitted until then. **Gate each task:** `just be-gate` — `cargo fmt --check` ·
> `cargo clippy --all-targets` (zero warnings) · `cargo test`, green **with and without**
> `--no-default-features`. Stop uncommitted; commit only at the phase review.

Four tasks, verify-early. Each names the library primitive it reuses (decided here, not during coding).

---

## Task 1 — Pure recapture computation

**Deliver** `crates/core/src/primitives/recapture/` (`mod.rs`): `RecaptureCredit`,
`RecapturePolicy`, `TokenValue`, and
`recapture_split(legs, prices, fill_spread, spread_token, policy) -> Vec<RecaptureCredit>`. Zero I/O.

**Formula (design §5).** Per leg, `leg_LVR_usd = value(amount_out @ token_out) − value(amount_in @
token_in)`; `rebate_usd = maker_share × leg_LVR` (positive legs only), aggregated per
`(maker, token_in)`, scaled down so the total ≤ `spread_cap × value(fill_spread @ spread_token)`,
dropped below `min_credit`, then converted to `token_in` base units. Direction falls out of
`RouteLeg` semantics: rebalancing (reverse) legs are positive; imbalancing (forward) legs compute
≤ 0 and are skipped.

**Reuses:** `RouteLeg` ([`primitives/routing`](../../../crates/core/src/primitives/routing/mod.rs)),
`UsdPrice::value` / `Usd` / `Bps` ([`primitives/shared/valuation.rs`](../../../crates/core/src/primitives/shared/valuation.rs)),
`rust_decimal`. **Fail-closed:** an unpriceable or out-of-range leg contributes nothing; the
function never errors.

**Tests** (`primitives::recapture::tests`): reverse leg credits the maker (design numbers — 1 ETH @
3000 vs 2700 USDC → 240 USDC @ 80%); forward leg → no credit; spread cap scales the total down;
dust floor drops a sub-`min_credit` credit; missing price skips the leg; multi-leg per-maker
aggregation.
**Re-run:** `cargo test -p solvent-core primitives::recapture`.

---

## Task 2 — Settlement wiring + E2E walking skeleton  *(verify-it-works milestone)*

**Deliver** the seam: `reconcile` surfaces confirmed `(intent, actuals)`; the app correlates
`intent → RoutePlan → RecaptureService` (`crates/core/src/recapture/service.rs`), which pulls oracle
prices, calls `recapture_split`, and accrues to an **in-memory** store (durable store is Task 3).

**Reuses:** the existing anvil e2e harness ([`crates/adapters/tests/e2e_*.rs`](../../../crates/adapters/tests))
+ `SelfHostedFeed`; the `PriceOracle` port; `SettlementReader`.

**Headline test** (hermetic): seed maker M → N forward `ETH→USDC` fills imbalance M → one reverse
`USDC→ETH` intent → assert (a) it routes to M, (b) a recapture credit accrues, (c) the resolver
nets ≥ 0.
**Re-run:** `cargo test -p solvent-adapters --test e2e_recapture <name>`.

---

## Task 3 — Durable `RecaptureStore`

**Deliver** port `crates/core/src/deps/recapture/store.rs` (`RecaptureStore` +
`RecaptureStoreError`) + sqlite adapter `crates/adapters/src/recapture/` + a `recapture_credits`
migration. Methods: `accrue`, `outstanding`, `mark_settled`.

**Reuses:** the existing sqlx/sqlite ledger-adapter pattern + `crates/adapters/migrations/`.

**Tests:** accrue / outstanding / mark_settled round-trip; per-intent idempotency (a re-run of
`reconcile` never double-credits).
**Re-run:** `cargo test -p solvent-adapters recapture`.

---

## Task 4 — `RebatePayer` + payout worker

**Deliver** port `crates/core/src/deps/recapture/payer.rs` (`RebatePayer` + `RebatePayerError`) + a
batched ERC-20 transfer adapter + the app payout loop (drain `outstanding` → `pay` → `mark_settled`).

**Reuses:** the execution tx-engine + `alloy` transfer encoding.

**Tests:** batching, `mark_settled` after confirmation, idempotent payout (a redelivered batch pays
once).
**Re-run:** `cargo test -p solvent-adapters payer`.

---

## Phase close

CLAUDE.md-standards refactor + fresh review over the whole phase (correctness **and** house rules);
`CHANGELOG.md` `[Unreleased]` updated before any PR; user reviews **uncommitted**; commit the phase
on approval. The Tier 1 (auction) concurrency work — extending
[`conserves_and_never_over_promises`](../../../crates/core/src/primitives/ledger/engine.rs) and the
auction-order races (design §6.5) — is the **next** phase, not this one.
