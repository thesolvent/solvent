# Solvent — a zero-inventory intent resolver on 1inch Aqua — Project Spec

> **Status:** living spec — single source of truth. **Target:** ETHOnline 2026 (async, Sept 4–16).
> Deep dives in `docs/`: `RESEARCH.md`, `CROSS_CHAIN_SETTLEMENT_PRIMER.md`, `RESOLVER_FLOW_CATALOG.md`, `ARCHITECTURE.md`.

---

## 1. TL;DR — the idea in two angles

- **Single-chain:** a **single Aqua-based maker balance backs solver inventory across many protocols at once** — 1inch, UniswapX, Across, CoW… — instead of each protocol needing its own siloed inventory. The capital stays **non-custodial** and does **double duty**; our resolver holds none of its own and keeps the spread.
- **Cross-chain:** use **Uniswap's The Compact** to lock the user's funds non-custodially on the **source** chain, and our **Aqua + SwapVM-backed solver** to deliver on the **destination** chain — a non-custodial cross-chain swap. Cross-chain messaging (e.g. **Chainlink CCIP**) is a **pluggable, optional** extension.

**We write one thin contract and one off-chain engine.** Pricing, settlement, the atomic guard, and order validation are all **audited 1inch + Uniswap infrastructure** we compose as a *taker*.

---



## 2. The problem

- **Idle capital** — ~85% of concentrated liquidity sat unused in H1 2026.
- **Solver inventory fragmentation** — solvers pre-position inventory per protocol/chain; a UniswapX filler's capital can't serve Across.
- **The destination-liquidity gap** — The Compact makes the *user's* side non-custodial, but the *filler* still fronts its own inventory.

**Thesis:** source filler liquidity from **non-custodial, double-duty maker capital via Aqua**, and keep the resolver **protocol-agnostic** so one liquidity layer serves many flows.

---



## 3. Scope


| Phase                         | Scope                                                                                                                                                               |
| ----------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Phase 1 (MVP — this spec)** | UniswapX resolver: same-chain, one protocol, end to end. Proves the thesis.                                                                                         |
| **Single-chain angle**        | One Aqua balance backing solver inventory across **many protocols** (1inch, UniswapX, Across, CoW…). Each protocol is a thin adapter over the same Aqua core.       |
| **Cross-chain angle (§10)**   | **The Compact** locks the user's funds on the source chain; our **Aqua/SwapVM solver** delivers on the destination chain. Cross-chain messaging (CCIP) is optional. |


---



## 4. Glossary — what 1inch gives us vs what we build


| Term                           | What it is                                                                                                                                                                                                                      | Owner   |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------- |
| **Aqua**                       | 1inch's non-custodial virtual-balance registry. `ship` books a *virtual balance* (an allowance, not custody); real tokens are pulled from the maker's wallet only at settlement — a pull beyond their real balance **reverts**. | 1inch   |
| **SwapVM**                     | 1inch's on-chain VM for swaps (opcodes: `staticBalances`, `limitSwap`, `xycSwap`, `dutchAuction`…).                                                                                                                             | 1inch   |
| **SwapVM program**             | A maker's pricing strategy compiled to **bytecode** (AMM curve, limit quote…). Not a contract — just bytes.                                                                                                                     | maker   |
| `SwapVMRouter`                 | The contract that **runs** programs: `quote()` (price) + `swap()` (execute + settle via Aqua). **It is the Aqua app** makers ship to.                                                                                           | 1inch   |
| **UniswapX Reactor / Permit2** | Uniswap's settlement + input-authorization: validates the signed order, pulls the input, calls the filler, delivers output.                                                                                                     | Uniswap |
| **Maker (LP)**                 | Runs a *normal 1inch position* — holds tokens, approves Aqua, ships a program. Passive; earns their curve's spread.                                                                                                             | actor   |
| **Swapper (user)**             | Signs a UniswapX order off-chain. Never touches Aqua.                                                                                                                                                                           | actor   |
| **Resolver (us)**              | Off-chain engine + thin filler contract + operator wallet. **Filler** to the swapper, **taker** to the maker; earns the price gap.                                                                                              | us      |
| `UniswapXAquaFiller`           | **The only contract we write** — a UniswapX filler that sources output from the router as a taker. No inventory, no direct Aqua calls.                                                                                          | us      |
| **Reservation ledger**         | Off-chain single-writer tracking each maker's available balance with TTL leases, so competing orders can't over-commit a maker.                                                                                                 | us      |


**The one rule:** the SwapVM router is the Aqua app and does all pricing + settlement — we never re-implement it, we call it as a taker.

---



## 5. Architecture — on-chain (the fill)

Our contract has two functions: `fillUniswapX` (engine starts a fill) and `reactorCallback` (reactor calls back mid-settlement). The callback is **three lines**: approve the router for the input, call `SwapVMRouter.swap()` to buy the output from the maker, approve the reactor for the output. **The router does the Aqua** `pull`**/**`push`**; we never touch Aqua.**

On-chain fill flow

*coral = UniswapX · teal = 1inch · purple = ours · gray = people. Steps 5–6 (maker settlement) are the router's, not ours.*

### 5.1 Worked example (grounded in `swapvm-lab` test 01)

*Maya* runs an **XYC AMM** over WETH/USDC, backed by ~100 WETH + ~290k USDC in Aqua (mid ≈2900; constant-product). *Alice* signs a UniswapX order resolving to **2960 USDC for 1 WETH**. One atomic tx:

1. Reactor pulls **2960 USDC** from Alice (Permit2) → filler.
2. `reactorCallback`: approve router → `swap(Maya's order, USDC→WETH, 1 WETH, threshold 2960)` → router pulls 1 WETH from Maya → us, takes **≈2930 USDC** from us → pushes to Maya → approve reactor.
3. Reactor pulls 1 WETH from filler → Alice.

**Result:** Alice −2960 USDC / +1 WETH · Maya −1 WETH / +≈2930 USDC · **Resolver +≈30 spread, 0 inventory.** If Maya lacked the WETH, step 2's `pull` reverts and the whole tx unwinds — we just lose the auction. The resolver never assumes the price; it reads `quote()` and keeps the gap.

---



## 6. Architecture — off-chain (the engine)

A Rust service that decides *which* fills to attempt and *never over-commits a maker* — a loop talking to the chain through **one RPC boundary** (reads to price/reconcile; one write — the fill).

Off-chain engine


| Component              | Responsibility                                                                                             |
| ---------------------- | ---------------------------------------------------------------------------------------------------------- |
| **Watcher**            | Ingests UniswapX orders → `NormalizedIntent`.                                                              |
| **Pricing**            | Calls `SwapVMRouter.quote()`; computes profit after gas.                                                   |
| **Reservation ledger** | Single-writer; `available = min(virtual cap, real balance) − leases`; TTL leases. **The novel core (§7).** |
| **Execution**          | Signs + sends the `fillUniswapX` tx.                                                                       |
| **Reconciler**         | On receipt, `settle`/`release` the lease; re-reads chain state to fix drift.                               |
| **Maker registry**     | Which positions exist (from Aqua `Shipped` events / config).                                               |


---



## 7. The reservation engine (the one hard, novel component)

On-chain over-draw is impossible (Aqua reverts). The real danger is **off-chain over-commitment**: committing to two orders that earmark the same maker balance → one settlement reverts → wasted gas (or slashing on bond protocols). The ledger prevents *promising* more than a maker can settle:

- **Single-writer, in-process**; reservations are **TTL leases** (ttl = order deadline).
- **Optimistic** where failure is cheap (UniswapX/7683); **pessimistic** where it's not (bond protocols).
- **Per-(chain, token) caps** = the utilization⇄safety dial.
- A **reconciler** corrects drift against chain state.

Core tension to surface to makers: tighter safety = lower utilization. That dial is the product.

---



## 8. Trust & failure model

- **Swapper** trusts only UniswapX — gets ≥ minimum output or the tx reverts.
- **Maker** trusts only Aqua + the router (audited, non-custodial) — funds move only at *their* price, capped by real balance. **Needn't trust us.**
- **We** risk only gas on a failed fill — no inventory, no custody.
- **Double-spend** blocked twice: off-chain by the ledger (saves gas), on-chain by Aqua (hard guarantee).

---



## 9. How external systems integrate

**Universal shape:** each adapter changes only the **order format**, the **fill entrypoint**, and the **claim**. The Aqua core never changes.

UniswapX + SwapVM router — reuse (we write none):

```solidity
struct SignedOrder { bytes order; bytes sig; }                       // order = DutchOrder; sig = Permit2
function executeWithCallback(SignedOrder order, bytes callbackData); // reactor entrypoint
function reactorCallback(ResolvedOrder[] orders, bytes data);        // our callback (IReactorCallback)
function quote(order, tokenIn, tokenOut, amount, takerTraits) view;  // router: price (order = maker program)
function swap (order, tokenIn, tokenOut, amount, takerTraits);       // router: execute + Aqua settle
```

Ours — the only new code:

```solidity
struct FillParams { ISwapVMRouter router; bytes makerOrder; address inputToken; address outputToken; uint256 maxInput; }
function fillUniswapX(SignedOrder order, FillParams p);   // engine → us
```

Adding a protocol = one adapter:


| Protocol                    | Order                    | Fill entrypoint               | Claim                         |
| --------------------------- | ------------------------ | ----------------------------- | ----------------------------- |
| **UniswapX** (P1)           | `SignedOrder` (Permit2)  | `Reactor.executeWithCallback` | atomic same-chain             |
| **ERC-7683**                | `GaslessCrossChainOrder` | `DestinationSettler.fill`     | atomic / settlement primitive |
| **Across**                  | `depositV3`              | `SpokePool.fillV3Relay`       | UMA optimistic (weak fit)     |
| **CoW / 0x-RFQ / Hashflow** | their quote              | their settlement              | atomic same-chain             |


---



## 10. Cross-chain (the frontier) — plainly

*Designed, not in the MVP. Deep dive:* `docs/CROSS_CHAIN_SETTLEMENT_PRIMER.md`*; steps:* `docs/RESOLVER_FLOW_CATALOG.md` *Flow D.*

**Why it's hard:** same-chain a fill is one atomic tx. Cross-chain, the user's money is on **chain A**, they want tokens on **chain B**, and no single tx touches both — so someone must **deliver on B first, get repaid on A after**, and bridge that gap safely.

**The mental model — two "locks", we're the courier:** a *resource lock* is a safe with a rule (opens only when a condition is met, else refunds). One per chain:

- **Destination B — the maker's Aqua lock:** liquidity we pull to pay the user (as in Phase 1).
- **Origin A — the user's Compact lock** ([The Compact](docs/CROSS_CHAIN_SETTLEMENT_PRIMER.md)): the user's input, which we claim *only after proving delivery*.

We hold neither side — pull from the maker's lock to pay the user on B, then claim the user's lock on A once delivery is proven. Aqua and The Compact are the *same shape* — non-custodial locks — one on each side.

```
   ORIGIN A (Base)                       DESTINATION B (Arbitrum)
 ┌─────────────────────┐              ┌──────────────────────────┐
 │ The Compact lock     │             │ Maker's Aqua position     │
 │ user's 1000 USDC     │             │ liquidity (non-custodial) │
 └─────────┬───────────┘             └────────────┬─────────────┘
           │ 4. claim (after proof)                │ 2. pull → deliver 0.3 ETH
           ▼                                        ▼
    ┌────────────────  Solvent (holds nothing)  ────────────────┐
    │  L3 proof (CCIP / optimistic)  ·  L4 netting + CCTP        │
    └───────────────────────────────────────────────────────────┘
                    3. proof of delivery: B → A
```

**Four layers, each one question:**


| Layer            | Question                            | Our choice                                            |
| ---------------- | ----------------------------------- | ----------------------------------------------------- |
| **L1 Liquidity** | Destination tokens from where?      | **Aqua** (what Phase 1 already does).                 |
| **L2 Claim**     | How is the filler repaid on origin? | **The Compact** resource lock.                        |
| **L3 Proof**     | How does origin learn of the fill?  | pluggable: CCIP / Hyperlane / LayerZero / optimistic. |
| **L4 Rebalance** | Move drifted inventory back         | netting (Everclear-style) + CCTP.                     |


> "The Compact vs CCIP" is a category error: Compact is **L2** (claim), CCIP is **L3** (the proof that fires it). They compose.

**Example:** Alice locks 1000 USDC on Base → we deliver 0.3 ETH on Arbitrum via a maker's Aqua position → proof travels B→A → we claim the 1000 USDC on Base → we repay the maker (netting). **Atomic for the user:** she gets her ETH or keeps her USDC, never neither; the *filler* carries the deliver-then-claim risk, minimized because the user's funds are pre-locked.

**Honest limit:** once we deliver on B, the maker's B-tokens are gone until value is rebalanced back — so the operator needs float or netting; "zero inventory" becomes "minimal inventory + netting." Aqua can't teleport tokens; netting *reduces* rebalancing, never eliminates it.

---



## 11. What's genuinely novel

Not shared liquidity (that's Aqua), not one-inventory-across-protocols (pro solvers do that with their own treasuries). **Ours:** a **non-custodial, passive-maker + reservation/adapter layer** — separating the capital-provider (a passive maker) from the operator (the resolver), packaged protocol-agnostically. The maker's destination liquidity becomes non-custodial and double-duty; the operator holds ~0 inventory.

---



## 12. Hackathon tracks


| Track                      | $        | How                                                            |
| -------------------------- | -------- | -------------------------------------------------------------- |
| **1inch — Build Aqua App** | 5,000    | resolver sources liquidity through Aqua/SwapVM                 |
| **Uniswap Foundation**     | 3,000    | fills UniswapX (P1); later The Compact — two Uniswap protocols |
| **The Graph**              | 15,000   | subgraph over positions + fills → utilization dashboard        |
| **Chainlink**              | 3,000    | CCIP as one L3 proof adapter                                   |
| **Privy / Ledger**         | 5,000 ea | maker onboarding / clear-signing                               |


Skipped (would be forced): Hedera, Arc, World, ENS.

---



## 13. Roadmap

1. **P0 — spike** ✅ proved a non-1inch contract can pull Aqua liquidity atomically (real Aqua contract).
2. **P1 — UniswapX MVP** — `UniswapXAquaFiller` + Rust engine (watch/price/reserve/execute/reconcile) + a sim whose headline is the **forced-contention test** (two orders, one maker balance → one granted, one declined off-chain, one fill).
3. **P2** — second adapter (ERC-7683): proves protocol-agnostic.
4. **P3** — utilization dashboard (The Graph) + multi-maker + risk caps.
5. **P4** — cross-chain (Aqua + Compact + proof adapter + netting).

---



## 14. Open decisions & Phase-1 done

**Decisions:** contract name `UniswapXAquaFiller`; off-chain **Rust** (hexagonal — `core` pure + ledger, `adapters` I/O, `engine` root); first sim driver Foundry/TS vs Rust; verify "fixed-rate + Aqua-backed" programs (XYC-AMM path proven).

**Phase-1 done when:** (1) `UniswapXAquaFiller` fills via the real router, no inventory; (2) the Rust engine runs the full loop on a local chain; (3) a sim reproduces §5.1 **and** the forced-contention test; (4) spec kept current, nothing merged without review.