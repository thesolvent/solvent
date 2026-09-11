# Solvent — a zero-inventory intent resolver on 1inch Aqua — Architecture

## 0. The project & the hackathon

### 0.1 The whole project (the vision)

**Solvent:** a **zero-inventory, protocol-agnostic intent resolver** built on 1inch Aqua. It fills orders from many intent protocols (UniswapX, ERC-7683, …) **without holding any capital of its own** — it borrows non-custodial *maker* capital through Aqua at the instant of settlement, delivers to the user, and keeps the spread.

**Why it matters (the problem):**
- **85% of concentrated liquidity sat idle** in H1 2026 — makers' capital is mostly dead weight between fills.
- **Intent solvers fragment inventory per protocol** — a UniswapX filler's capital can't serve Across, etc.
- **The destination-liquidity gap:** primitives like The Compact make the *user's* side of an intent non-custodial, but the *filler* still fronts its own inventory.

**The core idea:** the same maker balance does **double duty** — it backs the maker's own 1inch market-making *and* our resolver's fills across many protocols — collapsing the operator's working capital to ~0 and putting idle maker capital to work. Aqua's virtual balances are the atomic on-chain guard; an off-chain **reservation engine** stops over-commitment.

**Honest novelty (we don't overclaim):** Aqua already invented shared liquidity; pro solvers already run one treasury across protocols. What's genuinely ours is the **non-custodial, passive-maker + reservation/adapter layer** — separating the capital-provider (a passive maker) from the operator (the resolver), packaged protocol-agnostically. See `RESEARCH.md`.

### 0.2 What we're building for the hackathon

**Event:** ETHOnline 2026 (async, Sept 4–16, submit Sept 13).

**The build, in scope order:**
1. **Phase 1 — the UniswapX resolver (this document):** same-chain, one protocol, end to end. The MVP and the thesis proof.
2. **Extensibility — more adapters:** ERC-7683 (same standard, ~one adapter), then CoW / 0x-RFQ / Hashflow, etc. Each new protocol is a thin adapter over the same Aqua-sourcing core.
3. **Cross-chain frontier:** implemented for one configured EVM pair as two chain-local Solvent services plus a keyless proxy, using **The Compact**, destination **Aqua**, authenticated CCIP proofs, and direct or CCTP repayment. See `CROSS_CHAIN_OPERATIONS.md`.

**Tracks we target (all genuine, nothing forced):**
| Track | How we hit it |
|---|---|
| **1inch — Build Aqua App** ($5k) | the resolver sources liquidity through Aqua/SwapVM; the whole thesis is an Aqua use case |
| **Uniswap Foundation** ($3k) | fills **UniswapX** orders and uses **The Compact** for cross-chain — two Uniswap protocols |
| **The Graph** ($15k) | subgraph over Aqua positions + fills → the maker utilization dashboard |
| **Chainlink** ($3k) | CCIP as one adapter behind the cross-chain proof port |
| **Privy / Ledger** ($5k ea) | maker onboarding wallet / clear-signing |

**The layered model:**
- **L1 Liquidity** = Aqua (destination) — *Phase 1 uses this.*
- **L2 Claim** = per-protocol (UniswapX atomic same-chain; The Compact for the implemented cross-chain pair).
- **L3 Proof** = pluggable messaging (CCIP / Hyperlane / LayerZero / optimistic).
- **L4 Rebalance** = netting (Everclear-style) + CCTP.

---

# Phase 1: the UniswapX Resolver

**What we are building (Phase 1):** a **UniswapX resolver** — an off-chain engine plus one thin on-chain filler contract — that fills UniswapX orders using maker liquidity sourced *just-in-time* from 1inch Aqua through the SwapVM router. The resolver holds **zero inventory**: it owns no capital, borrows the maker's, and keeps the spread. It is same-chain, one protocol, and it proves the entire thesis end to end.

This document defines every component, who owns it, and exactly what talks to what. It is the reference the Phase 1 implementation plan builds against.

---

## 1. The actors (people / entities)

| Actor | Who they are | What they do |
|---|---|---|
| **Swapper** | an end user | Signs a UniswapX order off-chain ("I want 1 WETH, I'll pay USDC"). Never touches Aqua. |
| **Maker (LP)** | a liquidity provider | Runs a *normal 1inch position*: holds tokens, approves Aqua, and `ship`s a SwapVM program (their pricing strategy — an AMM curve, a limit quote, a pegged curve, etc.) to the SwapVM router. Passive; earns their curve's spread/fees. |
| **Resolver operator** | us | Runs the off-chain engine, owns the on-chain filler contract, and controls the wallet that submits fills and pays gas. Earns the gap between the order price and the maker's price. |

The resolver is **filler** to the swapper (UniswapX side) and **taker** to the maker (SwapVM side) — it sits in the middle.

### 1a. What "SwapVM" is (clearing the naming — it's all 1inch infra)

The maker's side runs entirely on **1inch's own infrastructure**; we build none of it. Three distinct things that are easy to conflate:

- **SwapVM** — 1inch's on-chain *virtual machine for swaps*: a set of opcodes (`staticBalances`, `limitSwap`, `xycSwap`, `dutchAuction`, `twap`, gating…). The instruction set.
- **A SwapVM *program*** — a maker's pricing strategy compiled to *bytecode* with the 1inch SDK (an XYC AMM curve, a limit quote, a pegged curve…). This is the maker's "position logic" — the thing they `ship`. **Not a contract** — just bytes.
- **`SwapVMRouter`** — the on-chain contract that *runs* programs. It exposes `quote()` (compute the price) and `swap()` (execute + settle via Aqua), and it is the Aqua "app" the maker ships to.

The maker never writes a contract — they compile a program and ship it. Our resolver never writes SwapVM — it only calls `quote()`/`swap()` as a taker. This is exactly the `swapvm-lab`'s 11 examples: a maker ships a program, a taker calls quote + swap. **The resolver is agnostic to which program the maker runs** — AMM, limit, pegged, whatever — it just uses the number `quote()` returns.

---

## 2. Components

### 2a. On-chain contracts

| Contract | Owner | Responsibility | We write it? |
|---|---|---|---|
| **`UniswapXAquaFiller`** | **us** | UniswapX filler. Implements `reactorCallback`; in it, sources the order's output from the SwapVM router as a taker, and repays nothing itself (the router does). Holds no inventory. | **YES — the only contract we write** |
| `Reactor` (e.g. `DutchOrderReactor`) | Uniswap | Validates + settles the UniswapX order; pulls input from swapper (Permit2); calls our filler; delivers output to swapper. | no (reuse) |
| `Permit2` | Uniswap | Lets the reactor pull the swapper's input against their signature. | no (reuse) |
| `SwapVMRouter` | 1inch | **The Aqua app.** Runs the maker's program (pricing) and performs Aqua `pull` (maker's output → us) + `push` (our input → maker). | no (reuse) |
| `Aqua` | 1inch | Virtual-balance registry. The **atomic double-spend guard** — a fill beyond the maker's real balance reverts here. | no (reuse) |
| ERC20s (USDC, WETH, …) | tokens | The assets moved. | no (reuse; mocks in tests) |
| Maker's **SwapVM position** | maker | *Data, not a contract* — the program shipped into the router that encodes the maker's price/curve and backs it with Aqua liquidity. | no (maker creates it) |

**Our on-chain footprint is one thin contract.** Everything hard (pricing, Aqua settlement, the atomic guard) is audited 1inch/Uniswap infra.

### 2b. Off-chain system (our "local system" — a Rust service)

This is the brain. It runs on the operator's machine/server and talks to an Ethereum RPC node.

| Component | Responsibility | Talks to |
|---|---|---|
| **Order watcher** | Subscribes to the UniswapX order feed; receives new signed orders. | UniswapX API (off-chain) |
| **Maker registry** | Knows which maker positions exist and their programs (from Aqua `Shipped` events / config), so we know what we can quote against. | RPC (read events) / config |
| **Pricing engine** | For an incoming order, calls `SwapVMRouter.quote()` (a read-only `eth_call`) against candidate maker positions; computes profit after gas. | RPC (`eth_call`) |
| **Reservation ledger** | Single-writer. Tracks each maker's `available = min(virtual cap, real balance) − in-flight leases`; grants or denies a reservation so two competing orders can't over-commit one maker. | in-memory (authoritative), reconciled from RPC |
| **Execution engine** | Builds, signs, and sends the `fillUniswapX` transaction; pays gas. | RPC (`eth_sendRawTransaction`) via the operator wallet |
| **Reconciler / tracker** | Watches fill receipts; on success `settle`s the lease, on revert `release`s it; periodically re-reads on-chain balances to correct ledger drift. | RPC (receipts, balances) |

### 2c. Infrastructure

- **Ethereum RPC node** (Anvil in simulation; a real node/L2 later) — the boundary between our off-chain system and the chain. Everything on-chain is reached through it.
- **Operator wallet** — the key that signs and pays for `fillUniswapX` transactions.

---

## 3. What interacts with what (the map)

```
        OFF-CHAIN (our Rust service)                         ON-CHAIN (via RPC node)
  ┌───────────────────────────────────────┐        ┌──────────────────────────────────────────┐
  │ Order watcher ─── new order ──┐        │        │                                          │
  │ Maker registry ─ positions ─┐ │        │        │   UniswapX Reactor  ── Permit2 (input)    │
  │                             ▼ ▼        │        │        │  ▲                               │
  │ Pricing engine ── quote() ──┼──────────┼──eth_call──────►│  │ reactorCallback               │
  │                             │          │        │        ▼  │                               │
  │ Reservation ledger ◄── reserve/settle  │        │   UniswapXAquaFiller (OURS)               │
  │                             │          │        │        │  swap()                          │
  │ Execution engine ── fillUniswapX tx ───┼──send──►│        ▼                                 │
  │                             │          │        │   SwapVMRouter ── pull/push ── Aqua       │
  │ Reconciler ◄── receipts/balances ──────┼──read──┤        │                                 │
  └───────────────────────────────────────┘        │        ▼        maker wallet ◄── push     │
                                                     │   ERC20s (USDC, WETH)                     │
        Swapper (signs order → UniswapX API)         │   Maker (ships program → SwapVMRouter)    │
        Operator wallet (signs the fill tx)          └──────────────────────────────────────────┘
```

**Read paths (off-chain → chain):** `quote()`, event logs, balances, receipts — all `eth_call`/reads through the RPC node.
**Write path (off-chain → chain):** exactly one — the `fillUniswapX` transaction.
**On-chain internal calls (one atomic tx):** Reactor → our Filler.reactorCallback → SwapVMRouter.swap → Aqua pull/push.

---

## 4. End-to-end example (worked, with numbers)

**Setup.**
- **Maya (maker)** runs an **XYC AMM** program (exactly `swapvm-lab` test 01) over WETH/USDC, backed *non-custodially* by ~**100 WETH + ~290,000 USDC** in Aqua — a mid-price of **≈2900 USDC/WETH**. Her price is **constant-product**: it moves slightly with fill size (no "flat" quote).
- **Alice (swapper)** wants **1 WETH** and signs a UniswapX Dutch order; at fill time it resolves to paying **2960 USDC** for 1 WETH, deadline in 60s.

**What happens.**
1. **Watch:** our Order watcher receives Alice's signed order from the UniswapX feed.
2. **Price:** the Pricing engine calls `SwapVMRouter.quote(Maya's order, USDC→WETH, 1 WETH)` → the XYC curve needs **≈2930 USDC** for 1 WETH (the 2900 mid + ~1% constant-product slippage on this size). The order pays 2960 → profit ≈ 30 USDC − gas → **fill it**.
3. **Reserve:** the Reservation ledger reserves 1 WETH against Maya's available balance, so a second order this block can't also promise the same WETH.
4. **Execute:** the Execution engine sends `fillUniswapX(Alice's signedOrder, {router, Maya's order, USDC, WETH, maxInput: 2960})`.
5. **On-chain, one atomic transaction:**
   - Reactor pulls **2960 USDC** from Alice (Permit2) → our Filler.
   - Reactor calls `OurFiller.reactorCallback`:
     - Filler approves the router for 2960 USDC.
     - `SwapVMRouter.swap(Maya's order, USDC→WETH, 1 WETH, threshold 2960)`:
       - router `pull`s **1 WETH** from Maya's Aqua position → Filler.
       - router takes **≈2930 USDC** from Filler → `push`es to Maya (her pool rebalances: WETH −1, USDC +2930).
     - Filler approves the reactor for 1 WETH.
   - Reactor pulls **1 WETH** from Filler → Alice.
6. **Reconcile:** the Reconciler sees the receipt and `settle`s the lease.

**Final ledger.**
| Party | Δ |
|---|---|
| Alice (swapper) | −2960 USDC, **+1 WETH** |
| Maya (maker) | −1 WETH, **+≈2930 USDC** (her XYC curve's price; the pool rebalances) |
| Resolver | **+≈30 USDC** spread, **0 inventory** (held nothing before or after) |

The exact number `2930` is whatever Maya's program computes — the resolver never assumes it, it reads `quote()` and keeps the gap. Whether the fill is exact-in or exact-out, and the precise slippage, are set by `takerTraits` + the curve. If Maya had held < 1 real WETH, step 5's `pull` reverts → the whole tx unwinds → we lose the auction, nobody is harmed. That's the atomic guard, for free.

---

## 5. Trust & failure model (Phase 1)

- **Swapper trusts** only UniswapX (audited) — they either get ≥ their minimum output or the tx reverts.
- **Maker trusts** only Aqua + the SwapVM router (audited, non-custodial) — funds leave their wallet only at the program's price, capped by their real balance. They need not trust us at all.
- **We (resolver) risk** only gas on a failed fill — no inventory, no custody.
- **Double-spend across competing orders** is prevented twice: off-chain by the reservation ledger (so we don't waste gas), and on-chain by Aqua (the hard guarantee).

---

## 6. Phase 1 deliverables (definition of done)

1. `UniswapXAquaFiller` — the one contract, filling via the real SwapVM router.
2. The off-chain engine: watcher (mock feed first) → pricing (`quote()`) → reservation ledger → execution → reconciler.
3. A simulation (Anvil) proving the worked example above end to end, plus the **forced-contention test**: two orders, one maker's last WETH — the ledger grants one, declines the other off-chain, and the fill lands exactly once.

Outside the original Phase 1 scope but now implemented separately: the configured-pair Compact/CCIP/CCTP cross-chain path. Still later: ERC-7683 and other protocol adapters, multi-pair netting, the Graph dashboard, and Privy onboarding.
