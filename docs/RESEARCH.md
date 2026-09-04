# Solvent — a zero-inventory intent resolver on 1inch Aqua — living research doc

**Status:** 🟢 Idea + cross-chain architecture finalized (§1, §5). Next: committed build spec.
**Target:** ETHOnline 2026 (Sept 4–16, async, submit Sept 13). Tracks: 1inch, Uniswap Foundation, The Graph, Chainlink, Privy/Ledger (§10).
**MVP decision (locked):** same-chain first; cross-chain design finalized as the Phase-4/stretch target.
**Author stance:** critical protocol-architect. Confirmed-from-source facts are marked ✅; design/speculation is marked ✎.

---

## 1. The idea (current best framing)

**What we're building.** A **zero-inventory, protocol-agnostic intent resolver**. It fills orders from multiple intent protocols (UniswapX + ERC-7683 first) while holding *none* of its own capital — it JIT-borrows non-custodial **maker** capital through **1inch Aqua** at the moment of settlement.

**Why.** The same maker balance can back the maker's own market-making *and* the resolver's fills across many protocols at once, so the **operator's working capital → ~0** and the **maker's idle capital starts earning** — with Aqua's on-chain `transferFrom` as the atomic anti-double-spend guard and an off-chain reservation engine preventing over-commitment.

**The liquidity provider (persona).** *Maya* runs a small MM desk, holds 1M USDC on Base, already quotes ETH/USDC on 1inch via an Aqua SwapVM strategy — but that 1M sits idle ~85% of the time between fills. She also `ship()`s a virtual balance to our resolver app. Now the same 1M simultaneously (a) backs her 1inch quotes and (b) is available to fill UniswapX/7683 orders the resolver wins on Base. She never moves or locks the money; whichever fires first pulls real USDC via Aqua's `transferFrom`, the rest don't. She earns resolver fees on capital that was already sitting there — no solver infra, no custody surrendered. If two fills race past her 1M, the second `transferFrom` reverts and the reservation engine had already declined to promise it.

### Honest novelty accounting
- ❌ "We invented shared liquidity" — false. Aqua **is** the shared-liquidity primitive.
- ❌ "First solver to use one inventory across protocols" — false. Pro solvers (Wintermute etc.) run one unified treasury across protocols already. "Siloed per-protocol inventory" is a strawman baseline.
- ✅ **Genuinely differentiated:** the resolver holds **zero inventory** — capital is the *maker's*, doing **double duty** (own quoting + resolver fills), pulled JIT; plus the **off-chain reservation/commitment engine** that makes one over-committable Aqua balance safe to promise across heterogeneous protocols whose "commit" (auction win) precedes settlement. The contribution is the **separation of capital-provider (passive maker) from operator (resolver)**, packaged protocol-agnostically.

### One-line pitch
> A protocol-agnostic, zero-inventory intent resolver: passive makers supply capital once through 1inch Aqua, and the resolver deploys that same non-custodial capital to fill orders across heterogeneous intent protocols, with Aqua's virtual balances as the atomic on-chain safeguard and an off-chain reservation engine coordinating commitments.

---

## 2. Load-bearing facts, confirmed from `Aqua.sol` source

These clear the idea-killer risks and reframe the "hard problem."

- ✅ **The "app" is just `msg.sender`.** `pull()` reads `_balances[maker][msg.sender][strategyHash][token]`. **There is no 1inch allowlist of apps** — any contract a maker `ship()`s to can pull their capital. → *A third-party resolver can be an Aqua app. Buildable by someone other than 1inch.*
- ✅ **`ship()` records a virtual allowance, not custody** (map comment: "aka makers' allowances"; no `transferFrom` in `ship`). Real tokens are pulled from the maker's wallet **at settlement** via `transferFrom(maker, to, amount)`. → *A maker can ship 1M to app A and 1M to app B holding only 1M. Aqua deliberately permits over-commitment.*
- ✅ **Double-spend is prevented atomically, on-chain.** The moment cumulative pulls exceed the maker's real balance/allowance, the next `transferFrom` reverts. **Aqua is itself the atomic inventory guard.**

**Consequence — the hard problem is reframed.** On-chain over-draw is *impossible* (revert). The real danger is **off-chain over-commitment**: winning/committing to two orders (auctions resolve independently, before settlement) that both earmark the same real balance → one settlement reverts → failed fill → gas burned, reputation hit, and on bond-based protocols, **slashing**. The reservation engine exists to avoid *promising* more than the shared real balance can settle.

---

## 3. Protocol-by-protocol compatibility

| Protocol | Filler permissionless? | Inventory / settlement model | Fit with Aqua JIT-pull | Verdict |
|---|---|---|---|---|
| **UniswapX** | ✅ deploy `fillContract`, implement `reactorCallback` | filler supplies own tokens; `executeWithCallback` invokes filler mid-settlement | **Excellent** — `reactorCallback` is the exact hook to JIT-`pull()` from Aqua atomically | **Primary target** |
| **ERC-7683** | ✅ standardized `open`/`fill` settlers | filler delivers output on destination | **Excellent** same-chain; **partial** cross-chain (§5) | **Primary target** |
| **Across** | ✅ permissionless race | relayer fronts own capital on destination, reimbursed *later* via UMA optimistic oracle | **Weak** — async reimbursement locks capital, diluting zero-inventory benefit | **Defer** — worst poster child |
| **1inch Fusion+** | ❌ permissioned (KYC/KYB, stake, Resolver NFT) | hashlock + timelock escrow on both chains (atomic HTLC) | **Native** (same trust domain) but gated | **Phase 2+** high-value adapter |

**Correction to the original pitch:** UniswapX + ERC-7683 (same-chain) are the right first targets because their fill is **atomic** — pull from Aqua and deliver in one tx, so Aqua's revert *is* the safety net. Across is the *wrong* first example.

---

## 4. The reservation / concurrency engine (same-chain)

On-chain safety is free (Aqua reverts). The engine solves **off-chain commitment**.

| Approach | How | Trade-off |
|---|---|---|
| **Single-writer reservation ledger** (core) | one authoritative in-process actor serializes reserve/release against `available = min(Aqua virtual, maker real) − reserved`; reservations are **TTL leases** = order deadline | simple, correct, fast; needs distributed lock for multi-instance |
| **Pessimistic reserve-before-commit** | never commit to a protocol order without a granted lease | safe; idle capital during leases → lower utilization. Tune TTLs tight |
| **Optimistic commit + graceful failure** | commit freely; treat settlement revert as expected | great where failure is cheap (UniswapX/7683 = just gas); **unsafe for bond protocols (Across)** |
| **Per-(chain,token) capacity caps** | cap each protocol's reservable share | re-silos slightly; the utilization⇄safety dial |
| **On-chain reservation/escrow** | pre-pull in resolver contract | adds gas+latency; usually unnecessary (Aqua already guards ceiling) |
| **Distributed lock** | Redis/etcd lease or shard by (maker,token) for N replicas | only at scale; start single-writer |

**Recommended synthesis:** single-writer, event-sourced `InventoryLedger` granting TTL leases; **optimistic** settlement for atomic/cheap-failure protocols, **pessimistic** leasing for bond-slashing ones; per-(chain,token) caps as the safety valve; a **reconciler** reading real Aqua/wallet state to correct ledger drift. The product's core tension to expose to makers: **tighter safety = lower utilization.**

---

## 5. Cross-chain architecture (FINALIZED design of record)

### 5.1 The key insight: Aqua and The Compact are the same shape, on opposite sides
[The Compact](https://blog.uniswap.org/the-compact-v1) (Uniswap) is a reusable, non-custodial **resource lock**: a user locks funds; an **allocator** prevents double-spend; an **arbiter** (cross-chain variant = **Tribunal**) releases the claim once delivery is proven. That is structurally identical to Aqua (a maker's virtual balance is a reusable non-custodial lock an app pulls from). Therefore:

> **The resolver is a zero-inventory value-router between two resource locks** — it `pull()`s from the **maker's Aqua lock** on the *destination* chain to deliver, and claims the **user's origin lock** once delivery is proven. It holds custody of neither side; the locks + their allocators/arbiters absorb the settlement-window risk, not the operator.

Aqua and cross-chain settlement are **orthogonal**: Aqua = *destination liquidity*; the origin claim = a *separate settlement primitive*. Compose them.

### 5.2 The architecture
```
ORIGIN (A): user input in a resource lock (The Compact)  ──claim after proof──┐
DESTINATION (B): maker's Aqua balance (passive per-chain LP) ──JIT pull()──►  │
                               Solvent       (holds ~nothing)                 │
  CrossChainSettlement PORT (pluggable):                                      │
    • resource-lock / Tribunal   • CCIP / Hyperlane / LayerZero attestation   │
    • Fusion+ hashlock escrow                                                 │
  Netting engine (bidirectional flow) + residual rebalancer (CCTP)  ◄─────────┘
```
The design decomposes into **4 orthogonal layers** — see [`CROSS_CHAIN_SETTLEMENT_PRIMER.md`](CROSS_CHAIN_SETTLEMENT_PRIMER.md) for the full teaching. The key correction: **The Compact (an L2 claim primitive) and CCIP (an L3 proof primitive) are different layers that compose — not alternatives.**
1. **L1 Liquidity = Aqua** — per-chain, passive, non-custodial makers who never take cross-chain risk.
2. **L2 Claim = per-protocol adapter behind a port.** **The Compact resource lock** is the best L2 *for flow we control* (our own intent flow, and 7683 orders opting into resource locks) — it's the same non-custodial-reusable-lock shape as Aqua, pre-locks the user's funds (minimal operator risk), and is proof-agnostic. For external protocols we adapt to *their* native L2 (Across→UMA optimistic, UniswapX→atomic same-chain). L2 is per-protocol, hence a port.
3. **L3 Proof = pluggable** (how origin learns the fill happened): CCIP (Chainlink track, RMN/institutional, ~10–20min), Hyperlane/LayerZero (fast, config-risk), or optimistic (cheap). Tribunal is explicitly proof-agnostic, so L3 is a free per-route choice.
4. **L4 Rebalance = bidirectional netting** ([Everclear](https://www.everclear.org/blog/everclear-powers-rebalancing-for-risk-labs-relayer-in-across-v4), settles user-visible in 5–30s) + **CCTP v2** fast transfer (USDC, 8–20s) for residuals.
5. **Lockup lives in the locks, not the operator.**

### 5.3 The one honest cost (zero-inventory degrades cross-chain)
Value flow: maker delivers 999 on B; resolver claims the user's 1000 on A; **the maker is now down on B and unpaid** — someone must repay them on B. Either cross-chain drift (bad — breaks "passive maker") or the resolver fronts/nets it. Therefore:
- **Same-chain: resolver is *truly* zero-inventory** (atomic; maker repaid in the same tx).
- **Cross-chain: resolver needs float *or* a netting/clearing layer** to promptly repay per-chain makers. Makers stay passive & non-custodial; the **operator absorbs rebalancing**. Zero-inventory becomes **"minimal-inventory + netting."**
- ✅ **Chain-level fragmentation is reduced by netting, never eliminated.** Real tokens must exist on the settlement chain. Distinguish **protocol-level fragmentation** (Aqua reduces) from **chain-level fragmentation** (netting reduces, does not remove).
- The reservation engine gains an **in-flight capital** state (delivered, awaiting claim/net).

### 5.4 Net position
- **Same-chain:** zero-inventory, complete win — atomic, no lockup, no rebalancing. (MVP.)
- **Cross-chain:** real but partial — non-custodial, passive, per-chain makers; operator runs minimal-inventory + netting; origin claim via a pluggable settlement primitive. Claim "reduces protocol-level fragmentation and destination-side capital needs, and nets cross-chain flow," **not** "eliminates rebalancing."

---

## 6. Recommended architecture (sketch)

```
   UniswapX API   7683 feed   (Across)   (Fusion+)
        │            │           │           │        OFF-CHAIN (Rust)
   ┌────▼────────────▼───────────▼───────────▼────┐
   │ Protocol Adapters → NormalizedIntent          │
   │ Pricing/Profit │ Risk (per chain/token)        │
   │ Inventory Manager ←→ Reservation Ledger (TTL)  │
   │ Execution planner │ State machine │ Reconciler  │
   └───────────────┬────────────────────────────────┘
                   │ signs & sends fill tx
   ┌───────────────▼────────────────────────────────┐  ON-CHAIN (Solidity, thin)
   │ UniversalResolver = an Aqua *app*               │
   │  - UniswapX fillContract/reactorCallback        │
   │  - 7683 settler fill entrypoint                 │
   │  - JIT AQUA.pull(maker, strategyHash, tok, amt) │ ← atomic guard
   │  - (cross-chain) fund destination escrow/settle │
   └───────────────┬────────────────────────────────┘
                   ▼   1inch Aqua → maker wallets (transferFrom at pull)
```
- **On-chain:** thin `UniversalResolver` app per chain; the chain's revert is the final guard.
- **Off-chain (Rust):** all intelligence. Shape: `tokio` async; event-sourced actor core (each order = an aggregate with an explicit FSM); `trait ProtocolAdapter` behind a port (hexagonal — matches evm-executor convention); a single authoritative `InventoryLedger` actor; idempotent replayable log for crash recovery. `NormalizedIntent` is the domain type.

```rust
struct NormalizedIntent {
    protocol: Protocol,
    input_chain: ChainId,   output_chain: ChainId,
    input_token: Address,   output_token: Address,
    input_amount: U256,     min_output_amount: U256,
    deadline: u64,
    settlement: SettlementRequirements, // oracle | hashlock | resource-lock | same-chain-atomic
}
```
(Even for the same-chain MVP, keep `input_chain`/`output_chain` in the model so cross-chain isn't a rewrite — matches "same-chain MVP, cross-chain-ready" if we choose it.)

---

## 7. Phased plan (draft)
- **P0 — feasibility spike:** on a fork, prove a non-1inch contract can be an Aqua app, `ship()` to it, and `pull()` to settle a mock UniswapX fill. Empirically confirms §2 end-to-end.
- **P1 — MVP:** UniswapX, single-chain, optimistic settlement, single-writer reservation ledger + reconciler + **forced-contention test** (second settlement reverts cleanly, ledger recovers). *That test is the proof of thesis.*
- **P2 — second adapter:** ERC-7683 same-chain (proves "protocol-agnostic" with a genuinely different order format normalizing to the same core).
- **P3 — risk & multi-maker:** per-chain/token caps, multiple makers, utilization dashboard (maker-facing product); **P1 simulation** replaying historical order flow to *measure* capital savings (not assert them).
- **P4 — cross-chain frontier:** pick a settlement primitive (Aqua+7683-oracle and/or Aqua+Fusion+-hashlock), add in-flight capital state, chain-positioning/rebalancing, multi-instance locking.

## 8. Principal risks
- **Cold-start (biggest):** no makers → no pool. Edge is non-custodial passive-maker capital + utilization, **not** latency.
- **Latency:** intent auctions are races; reservation hops must stay in-process/hot or you lose auctions.
- **Adverse selection:** you win the orders worst for the maker; the maker's Aqua quote must be the price floor.
- **Normal-revert reputation:** over-commitment makes settlement reverts routine — fine on UniswapX/7683, dangerous on bond protocols.
- **Aqua dependency + version skew** (already witnessed in the SwapVM lab).
- **Regulatory:** pooling third-party capital as a resolver service may have money-transmission surface; Fusion+ needs KYC.

## 9. Open questions to finalize
1. Same-chain-only MVP model, or same-chain MVP with a cross-chain-ready `NormalizedIntent`? (leaning: cross-chain-ready shape, same-chain execution)
2. First adapter: UniswapX or ERC-7683? (leaning: UniswapX for the atomic `reactorCallback` hook; 7683 as P2)
3. Cross-chain settlement primitive to target first when we get there: **7683-oracle** (permissionless) vs **Fusion+-hashlock** (in-domain, atomic, but KYC)?
4. Can we get *any* real order-flow data to make the capital-savings claim quantitative, or is P3 simulation the only honest path?
5. Is "resolver-as-a-service pooling third-party capital" a regulatory non-starter that forces a "solver runs it on their own makers' capital" framing instead?

---

## 10. ETHOnline 2026 track alignment (Sept 4–16, submit Sept 13, async)

Targeting the endorsed stack — each fit is genuine, no forcing. (Skipped: Hedera/Arc/World/ENS — would be forced.)

| Track | $ | How we hit it (genuinely) |
|---|---|---|
| 🎯 **1inch — Build Aqua App** | $5,000 | The project *is* a sophisticated Aqua app: a resolver that `ship()`s/`pull()`s Aqua as shared inventory. Home track. Demo must show the app mechanics + forced-contention test. |
| 🎯 **Uniswap Foundation — Best Stack** | $3,000 | **Double** integration: UniswapX (order-flow adapter #1) + **The Compact** (origin resource lock, §5). Both are Uniswap protocols. |
| 📊 **The Graph** | $15,000 | Subgraph over Aqua `ship/pull/push` + resolver fills + Compact claims → powers the reconciler and the maker utilization dashboard (visual wow). Needed anyway. |
| 🔗 **Chainlink** | $3,000 | CCIP as **one adapter** behind the `CrossChainSettlement` port (fill-proof attestation), and/or price feeds in the profitability engine. Not the architecture — one option. |
| 🟣 **Privy** | $5,000 | Maker onboarding / embedded wallet in the utilization dashboard UI. |
| 🔑 **Ledger** | $5,000 | EIP-712 clear-signing for maker `ship()` and resolver ops (optional polish). |

**Demo requirements to actually qualify:** (1inch) visible ship→pull + forced-contention test; (Uniswap) a real UniswapX order filled via Aqua pull + The Compact origin lock; (Graph) utilization dashboard from a subgraph; (Chainlink) a CCIP fill-proof relay; (hygiene) 2-min video + README + the "Maya's idle 1M does double duty" narrative.

## Sources
- 1inch Aqua: [CoinDesk](https://www.coindesk.com/web3/2026/07/27/1inch-opens-aqua-liquidity-protocol-across-13-chains) · primary: `aqua-repo/src/Aqua.sol` (this study)
- UniswapX fillers: [Uniswap docs](https://docs.uniswap.org/contracts/uniswapx/fillers/mainnet/createfiller)
- ERC-7683: [EIP-7683](https://eips.ethereum.org/EIPS/eip-7683) · [erc7683.org spec](https://www.erc7683.org/spec) · flow: [Eco deep dive](https://eco.com/support/en/articles/14796366-erc-7683-cross-chain-intents-standard-deep-dive)
- Across: [intents architecture](https://docs.across.to/guides/concepts/intents-architecture) · [Across Prime](https://www.paradigm.xyz/2025/05/across-prime)
- Fusion+ (hashlock/escrow): [1inch help](https://help.1inch.com/en/articles/9842591-what-is-1inch-fusion-and-how-does-it-work) · [cross-chain-swap repo](https://github.com/1inch/cross-chain-swap) · resolver onboarding (KYC): [business.1inch](https://business.1inch.com/portal/documentation/resolvers/introduction)
