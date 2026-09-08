# Routing harness — design

> **Status:** design of record, awaiting review. **Target:** ETHOnline 2026 (submit Sept 13).
> Scope: a testing harness that validates the routing engine under realistic conditions and
> doubles as the demo. Backend phases (B0–B4) are prerequisites, not part of this document.

---

## 1. Why this exists

The routing engine (`crates/core/src/routing/`) is the system's most complex component and its
correctness claims are currently made against **synthetic liquidity**: `routing_experiment.rs`
seeds pools from a SplitMix64 RNG, `benches/routing.rs` times solves over generated candidates,
and `e2e_routing.rs` runs against an anvil chain whose Aqua state we ship ourselves.

That is enough to prove the solver is *internally* correct — the property suite is thorough — but
it cannot answer the questions that decide whether the thesis holds:

- Does the water-fill produce **good splits against liquidity that actually exists**?
- Does `per_leg_cost` **match real gas** on the real reactor and the real router?
- What fraction of real order flow is **routable at a profit** at all?
- Does the solve stay inside its latency budget on **real candidate sets**?

This harness answers those four questions with numbers, and renders the answers.

### What already exists (not rebuilt)

| Asset | Covers |
|---|---|
| `crates/core/tests/routing_properties.rs` | invariants, optimality oracles, extreme-scale robustness |
| `crates/core/tests/routing_experiment.rs` | funnel/K study vs the all-pools optimum, seeded |
| `crates/core/benches/routing.rs` | solve latency p99 vs a 500 ms budget |
| `crates/adapters/tests/e2e_*.rs` | hermetic anvil loop: sync → route → reserve → fill |
| `SelfHostedFeed` | mints and signs UniswapX orders in-process |

The harness reuses all of it. The gap it closes is **real liquidity, real order distribution, a
real gas yardstick, and visualization**.

---

## 2. Substrate

**A Tenderly Virtual TestNet forked from Ethereum mainnet, with state-sync.** This is the
environment — not a stepping stone to one.

### What the fork already contains, real

| Contract | Address |
|---|---|
| UniswapX `V2DutchOrderReactor` | `0x00000011F84B9aa48e5f8aA8B9897600006289Be` |
| Permit2 | canonical |
| Aqua registry | `0x1111113ccf1426a8e30e2bff5e005d929bf6a90a` |
| SwapVM router | `0x111111338c5091e8440b67b168bae16a668ac0de` |

`UniswapXAquaFiller` is the only contract we deploy.

### Why a fork and not a public testnet

Neither UniswapX nor Aqua has any testnet deployment — UniswapX lists 9 mainnet chains and no
testnets; SwapVM's own broadcast artifacts cover chain ids 1, 10, 56, 100, 130, 137, 146, 324,
4663, 8453, 42161, 43114, 59144, all mainnets. On a public testnet every pool's depth would be a
number we chose, so every routing measurement would be measuring our own fixture. The fork
inherits real maker state, real curve shapes, real depths and real prices.

### The one fabricated step, stated plainly

Real makers exist on mainnet Aqua, but none have `ship()`-ed to our filler — the contract is new.
So the harness reads makers' **real strategy programs and real balances** from forked Aqua state,
then impersonates those makers to ship the *same* programs to our app at the *same* sizes.

Real: maker identities, curve types, curve parameters, depths, token balances, prices.
Fabricated: the ship-to-our-app relationship.

This is the honest ceiling without mainnet adoption, and it is disclosed in the run output rather
than buried.

### What the fork does not model

Instant-mined blocks, no mempool competition, no reorgs. Latency figures are therefore **solver
latency**, not end-to-end race latency, and are reported as such. Gas is real (real bytecode,
real storage), block timing is not.

Local anvil remains the CI substrate; existing `e2e_*.rs` tests are untouched.

---

## 3. Order corpus

Two sources, different jobs.

### 3.1 Captured corpus — realism and ground truth

A capture binary, run **once**, writing a committed fixture:

1. Scan mainnet `Fill(bytes32 orderHash, address filler, address swapper, uint256 nonce)` logs
   over a bounded window.
2. The event carries no amounts, so for each hit fetch the filling transaction and decode:
   - **calldata** → the signed order (tokens, amounts, decay parameters, deadline)
   - **ERC-20 `Transfer` events in that tx** → what the swapper actually received
3. Record order + realized output + filler + block + gas used.

The realized output is the yardstick: *what did the winning filler actually deliver, and would we
have beaten it?* No other source provides that.

Bounded to a few hundred fills — enough for a size/pair distribution and a benchmark, small
enough to commit and replay offline forever. Once captured, the harness never depends on any
external API, which also removes the demo's dependence on Uniswap's (API-key-gated, partly
deprecated) Orders API.

### 3.2 Synthetic specs — edges and on-demand orders

`SelfHostedFeed`, already built, covers what mainnet happens not to contain: dust legs,
cap-bound trades, deliberately contended orders, sizes past the deepest maker. It also supplies
orders on demand for the live demo, where waiting for a replay window is not an option.

---

## 4. Scenarios and metrics

| Metric | Definition | Measured how |
|---|---|---|
| **Speed** | `route()` wall-clock, p50/p99, per funnel size | `Instant` around the solve, real candidate sets from forked Aqua state |
| **Cost** | gas actually burned by the real reactor + filler, **vs `per_leg_cost`'s prediction** | execute the fill on the fork, read the receipt, diff against the model |
| **Routing efficiency** | our net output vs (a) the winning filler's realized output, (b) the all-pools brute optimum | corpus ground truth; `solve` over the unfunneled candidate set |
| **Solvability** | share of corpus orders routable at a profit, declines bucketed by cause | no-liquidity / cap-bound / gas-unprofitable / below-taker-bound |

**Cost is the highest-value metric here.** `per_leg_cost` currently converts gas → native → USD →
spread token with no empirical check that its gas-unit constant matches the deployed filler. A
wrong constant silently mis-tunes the sparsity pass in both directions: too high and profitable
splits get collapsed, too low and dust legs survive. This is the first thing the harness should
falsify.

**Contention scenario:** many concurrent orders competing for shared maker wallets — the
forced-contention test the ledger was built for, at corpus scale, asserting the invariant that
holds the whole thesis together: every routed plan is simultaneously reservable.

---

## 5. `RunResult` — the schema is the contract

One schema, produced by both drivers, consumed by the frontend.

**Per run:** config (fork block, corpus id, `RoutingConfig`), environment (substrate, disclosed
fabrications), aggregate metrics, error log.

**Per order:** the request; the candidate set with each pool's curve type, depth and caps; the
**λ** the bisection settled on; the chosen split per leg; legs dropped by the sparsity pass and
why; expected vs realized profit; the ground-truth comparison; solve latency; gas predicted vs
gas burned.

The X-ray view is a direct rendering of one such record. The schema is designed for that, which
is why it is specified before either driver.

---

## 6. Architecture

```
crates/harness/              new crate — depends on core + adapters; app does NOT depend on it
  src/
    env/                     fork setup, filler deploy, maker mirroring
    corpus/                  capture + fixture load; synthetic specs
    scenarios/               the suite
    scoring/                 pure: efficiency, solvability, latency, gas delta
    result.rs                RunResult + serde
  bin/
    capture.rs               one-shot: mainnet logs -> committed fixture
    batch.rs                 run suite -> RunResult JSON
    serve.rs                 same core, live over HTTP/SSE
web/                         Vite + React + TS, static-deployed, reads RunResult
```

**Two drivers, one core.** `batch` produces reproducible numbers for validation and CI; `serve`
runs the identical scenario core live for the demo. The frontend reads the same schema from a
static file or a live stream with no code change.

**Crate, not a feature flag.** CLAUDE.md reserves a `backtest` cargo feature so the live binary
never compiles heavy backtest dependencies. A separate crate achieves that more directly:
`crates/app` simply does not depend on `crates/harness`, so nothing heavy reaches the live binary
and no feature gate is needed.

**House rules apply.** The harness is real code, not a scratch script: ports one-per-file with
their own `{Trait}Error`, public fallible APIs return `SolventError`, no `unwrap`/`expect` outside
`#[cfg(test)]`, gate green with and without `--no-default-features`.

---

## 7. Frontend

**Vite + React + TypeScript, static-deployed**, no backend of its own.

**Dashboard** — the order stream/list plus aggregate metrics: solvability with declines broken
out by cause, the efficiency distribution against the winning-filler benchmark, latency p50/p99,
and predicted-vs-actual gas.

**X-ray drill-down** — click an order to see the solve itself: candidate pools as depth bars, the
λ level drawn across them, the split that landed, legs greyed where gas killed them, and the
resulting spread against what the winning filler achieved.

The X-ray is hand-rolled SVG. The water-fill visual is bespoke — a chart library does not help
and would fight the layout. The summary strip can use a light chart dependency.

Sequenced X-ray first: it is the differentiated artifact and it works off a static `RunResult`,
so it is complete and demoable before `serve.rs` exists.

---

## 8. Failure handling

- **Substrate or server down during judging** → the frontend falls back to the last recorded run,
  committed as a static artifact. The demo never hard-depends on a live service.
- **RPC failure mid-run** → recorded in `RunResult.errors` and the scenario continues; a partial
  run reports honestly rather than aborting.
- **Corpus capture is one-shot and offline** → no run ever depends on mainnet API availability.
- **Fork state drift** (state-sync moves mainnet under us) → every run pins and records its fork
  block, so results stay reproducible and comparable.

---

## 9. Testing the harness

It is test infrastructure that can itself lie, so it earns the same bar:

- **Scoring functions are pure** → unit-tested directly, including the degenerate cases
  (zero-leg plans, declines, missing ground truth).
- **One golden `RunResult` snapshot** → catches schema drift, which would silently break the
  frontend.
- **No tests** for serde derives, config plumbing, or CLI wiring, per house rules.

---

## 10. Non-goals

- No live mainnet order feed — API-key-gated and partly deprecated; the captured corpus replaces it.
- No public-testnet deployment — measured routing numbers there would be measuring our own fixture.
- No cross-chain, no multi-instance/distributed harness, no backtest beyond the captured window.
- No mempool/race simulation — the fork cannot model it, so the harness does not pretend to.

---

## 11. Sequencing

| # | Deliverable | Unlocks |
|---|---|---|
| 1 | Fork env + filler deployed + makers mirrored | substrate proven |
| 2 | Corpus capture → fixture committed | real orders + ground truth |
| 3 | Batch runner + scoring | **real numbers exist; validation goal met** |
| 4 | X-ray frontend on static `RunResult` | **the differentiated artifact exists** |
| 5 | `serve.rs` + live dashboard | the "alive" layer |
| 6 | Contention + stress scenarios | thesis invariant at scale |

Steps 1–4 satisfy the validation goal and produce the money shot. Step 5 is the first cut if the
calendar tightens, and cutting it costs nothing structurally because the frontend already runs off
static data.

---

## 12. Risks

| Risk | Mitigation |
|---|---|
| Calendar — 11 days, with backend phases still in flight | strict ordering above; steps 5–6 are droppable without restructuring |
| Tenderly free-tier limits unknown at design time | verify before step 1; a self-hosted persistent anvil fork is a drop-in fallback (same RPC surface, loses the hosted explorer) |
| Tx decoding for corpus capture is fiddlier than expected | fall back to `Transfer`-events-only for realized amounts; the signed order can be reconstructed from the reactor's calldata alone |
| Mirrored makers are not a real ship relationship | disclosed in `RunResult.environment` and in the UI, not hidden |
