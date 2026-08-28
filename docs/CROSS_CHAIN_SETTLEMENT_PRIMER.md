# Cross-chain settlement, taught from first principles — and why The Compact wins *for our layer*

This is knowledge transfer, not just a decision log. By the end you should be able to reason about any cross-chain design by decomposing it into **four orthogonal layers** and asking "what does each layer use, and why."

The headline correction (I made this mistake earlier too): **"The Compact vs CCIP" is a category error.** They are not competitors — they sit on *different layers* and *compose*. Let's build the mental model that makes that obvious.

---

## 1. The mental model: every cross-chain intent fill is 4 orthogonal layers

A user has input on chain A, wants output on chain B. A resolver fills it. Four separable questions must each be answered:

| Layer | The question it answers | For our resolver |
|---|---|---|
| **L1 — Liquidity** | Where do the *destination* tokens come from to pay the user on B? | **Aqua** (JIT-`pull()` the maker's balance on B). Solved by our core. |
| **L2 — Claim / authorization** | How is the filler *guaranteed to be repaid* the input on A after delivering on B? | The layer we're choosing. Escrow / optimistic / HTLC / **resource lock**. |
| **L3 — Proof of fill** | How does chain A *learn* that the fill on B actually happened? | Messaging (CCIP / LayerZero / Hyperlane) / optimistic assert / storage proof. |
| **L4 — Rebalancing** | After many fills, inventory drifts across chains. How is the *residual* moved back? | Netting (Everclear) + canonical bridge (CCTP). |

**The single most important thing to internalize:** L2 and L3 are *different layers*. The Compact is an **L2** primitive (it defines the lock and the claim). CCIP/LayerZero/Hyperlane are **L3** primitives (they carry the proof that lets the L2 claim fire). A real system picks *one from each layer*. So you don't choose "Compact **or** CCIP" — you choose "Compact (L2) **with** CCIP-or-optimistic-or-storage-proof (L3)."

Confirmed from source: Tribunal (The Compact's cross-chain arbiter) is deliberately **proof-agnostic** — "external bridge protocols extend Tribunal by overriding internal functions" to pass the message / let the arbiter pull it. That's L2 explicitly leaving L3 pluggable.

---

## 2. Layer L2 — the Claim / authorization primitives (this is the real choice)

What guarantees the filler gets paid on the origin chain? Four families, taught in order of increasing elegance for our case.

### 2a. Traditional escrow (deposit → release on proof)
User deposits input into an escrow contract on A; it releases to the filler once a proof of fill is presented. Simple. But: single-purpose (one escrow per order), often the escrow custodies funds, and it doesn't compose or get reused. Capital sits idle in escrow. **Baseline; superseded by resource locks.**

### 2b. Optimistic oracle (Across / UMA)
The filler *fronts their own capital* on B, then requests reimbursement from an origin pool; a bonded **optimistic oracle** assumes the claim is valid unless a challenger disputes within a window. 
- **Trust:** economic (bonded challengers watch).
- **Latency:** user paid in seconds; **filler reimbursement waits the dispute window** (minutes+).
- **Capital:** the filler's capital is locked for the whole window. *This is exactly what dilutes our zero-inventory benefit.*
- **Who bears risk:** the filler (front + wait + slashing on invalid fills).

### 2c. Hash-time-locked contracts / HTLC (1inch Fusion+, Garden)
Escrows on **both** chains bound by a hashlock + timelock. The filler funds a destination escrow; revealing a secret atomically unlocks the destination for the user and lets the filler claim the origin escrow with the same secret. 
- **Trust:** cryptographic + **liveness** (someone must reveal before the timelock, or it refunds).
- **Latency:** two escrow deposits + reveal round-trip.
- **Capital:** locked in escrows **on both chains** during the timelock — the heaviest capital profile.
- **Who bears risk:** filler + a liveness assumption. Atomic (no external bridge trust), which is its virtue.
- **Note:** Fusion+ is KYC-gated; same trust domain as Aqua, so it's a natural *optional* adapter, not the default.

### 2d. Resource locks (Uniswap's The Compact) — the one we want
An ownerless ERC-6909 contract holding **reusable, non-custodial locks**. A user (sponsor) locks funds once; an **allocator** co-signs claims and **prevents under-allocation** (double-spend of the lock); an **arbiter** verifies the fill conditions and releases the claim. **Tribunal** is the cross-chain arbiter.
- **Trust:** *choosable* — from fully on-chain allocator (trustless) to hybrid (fast). The sponsor picks their security/speed tradeoff.
- **Latency:** fast — the allocator co-signs; no long dispute window inherent.
- **Capital:** the **user's** funds are locked upfront, and the lock is **reusable** (not one-shot escrow). The filler takes *minimal* claim risk because the claim is near-guaranteed once it delivers.
- **Who bears risk:** minimized and pushed into the lock + allocator, not the operator.

---

## 3. Layer L3 — the Proof-of-fill mechanisms (pluggable under L2)

How does the origin chain *know* the destination fill happened? This is where CCIP et al. live. The L2 claim can't fire without an L3 proof.

| Mechanism | How it proves | Trust model | Latency | Notes |
|---|---|---|---|---|
| **Chainlink CCIP** | DON attests the destination event; independent **Risk Management Network** double-checks and can halt | fixed committee + RMN (defense-in-depth) | **deliberately slow, ~10–20 min** (risk checks) | clean exploit record; institutional; **this is the Chainlink track** |
| **LayerZero** | configurable **DVNs** (Required + Optional) attest | per-app (you pick DVNs) | 30s–min with fast DVNs | flexible but **config-risk** (KelpDAO ~$292M, Apr 2026, 1-of-1 DVN misconfig) |
| **Hyperlane** | modular **ISMs** (multisig + optimistic aggregation) | per-app | 30s–min | permissionless, 150+ chains, 7 VM types |
| **Optimistic assert** | assert-and-challenge window (à la UMA) | economic | minutes (window) | cheapest; adds latency |
| **Storage proof / light client** | prove the destination state on origin cryptographically | trust-minimized | slower / costlier | strongest trust, heaviest cost |

**Teaching point:** fixed-committee (CCIP, Wormhole) = predictable but concentrated trust; modular (LayerZero, Hyperlane) = flexible but only as safe as its configuration. There is no free lunch — you trade committee-concentration against config-surface.

---

## 4. Layer L4 — Rebalancing (close the residual, orthogonal to everything above)
Even with perfect L1–L3, real tokens drift toward origin chains over many fills. Two tools, used together:
- **Bidirectional netting / clearing (Everclear):** opposite-direction flows cancel; only the *net* imbalance ever needs a bridge. Solver-netted intents settle user-visible in 5–30s.
- **Canonical rebalance (CCTP v2):** burn-and-mint **native USDC**, **Fast Transfer settles in 8–20s** (faster-than-finality via early attestation), 13+ chains, tiny bp fee. The clean rail to move the residual.

---

## 5. Our evaluation criteria (why "best" is use-case-specific)

Rank the L2 options against what *our* resolver actually needs:
1. **Keep makers passive & non-custodial** — they must not run infra or take exotic risk.
2. **Minimize operator capital lockup & cross-chain risk** — preserve "minimal-inventory."
3. **Speed** — intent auctions are races.
4. **Trust-minimization** — don't bolt on a systemic weak link.
5. **Composability with Aqua** — ideally the *same shape*.
6. **Permissionless** — no KYC gate for the open version.
7. **Bonus: proof-agnostic** — so L3 stays a free choice (and we can pick CCIP for the track).

| L2 option | Passive makers | Min operator lockup | Speed | Trust-min | Composes w/ Aqua | Permissionless | Proof-agnostic |
|---|:--:|:--:|:--:|:--:|:--:|:--:|:--:|
| Escrow | ~ | ✗ (idle in escrow) | ~ | ~ | ✗ | ✓ | ~ |
| Optimistic (Across) | ✓ | ✗ (window lockup) | user fast / claim slow | ~ (economic) | ~ | ✓ | tied to UMA |
| HTLC (Fusion+) | ~ | ✗ (both-side escrow) | ~ (round-trip) | ✓ (atomic) | ~ | ✗ (KYC) | ✗ (self-contained) |
| **Resource lock (Compact)** | **✓** | **✓ (user pre-locked)** | **✓** | **✓ (choosable)** | **✓✓ (same shape)** | **✓** | **✓ (Tribunal)** |

---

## 6. Verdict — why The Compact wins **at L2**, precisely

1. **It is the same shape as Aqua.** Aqua = a reusable, non-custodial lock the *maker* provides on the destination; The Compact = a reusable, non-custodial lock the *user* provides on the origin. Our resolver becomes a **zero-custody value-router between two symmetric locks**, each with its own anti-double-spend guard (Aqua's `transferFrom` revert on one side; the allocator on the other). That symmetry is not cosmetic — it means neither side needs to trust the operator with custody.
2. **It minimizes operator lockup.** The user's funds are pre-locked, so the filler delivers knowing the claim is near-guaranteed — no multi-minute reimbursement window tying up operator capital (the Across weakness), no both-chain escrow capital (the HTLC weakness).
3. **It keeps L3 free.** Tribunal is proof-agnostic, so we plug in CCIP (Chainlink track / institutional), or a fast DVN/ISM for UX, or optimistic for cheapness — *per route*, without re-architecting. The others fuse L2+L3 (Across→UMA, Fusion+→its own hashlock), removing that freedom.
4. **It's permissionless, standard, and a Uniswap protocol** (bonus: strengthens the Uniswap track alongside UniswapX).

## 7. The honest caveat you must hold (this is the real nuance)

**The Compact only applies when the order *originates* inside a resource lock.** Our thesis is *protocol-agnostic*, and third-party protocols dictate their **own** L2:
- **UniswapX / same-chain 7683:** atomic same-chain settlement — no cross-chain L2 needed; Aqua just provides liquidity.
- **Across:** its L2 *is* the UMA optimistic oracle. To fill Across flow, we adapt to *that*, not Compact.
- **ERC-7683 cross-chain:** settlement-agnostic — the settler can choose escrow, optimistic, or **resource-lock**. Here we *can* use Compact.
- **Our own originated intent flow:** we control it, so we choose **Compact** (best).

So the precise claim is: **The Compact is the best L2 primitive for the settlement *we* control (our own cross-chain intent flow, and 7683 orders that opt into resource locks); for external protocols we adapt to their native L2 via per-protocol adapters.** This is exactly why the architecture needs a `CrossChainSettlement` **port** — L2 is per-protocol, not global.

---

## 8. The finalized layered architecture

```
              chain A (ORIGIN)                         chain B (DESTINATION)
  L2 claim ┌──────────────────────────┐    L1 liq ┌──────────────────────────┐
           │ The Compact resource lock │           │ Aqua maker balance       │
           │  (user funds, allocator)  │           │  (passive per-chain LP)  │
           └───────────┬──────────────┘            └───────────┬──────────────┘
                       │ claim released on proof                │ JIT pull() to deliver
                       ▼                                        ▼
        ┌────────────────────────────────  Solvent  ────────────────────────────────┐
        │  per-protocol L2 ADAPTERS: Compact | UMA(Across) | UniswapX-atomic | 7683 │
        │  L3 PROOF port: CCIP (Chainlink) | Hyperlane/LZ (fast) | optimistic       │
        │  L4 REBALANCE: Everclear-style netting  +  CCTP v2 fast transfer (USDC)    │
        └───────────────────────────────────────────────────────────────────────────┘
```
- **L1** Aqua — maker destination liquidity (our core).
- **L2** per-protocol claim adapter; **Compact** for flow we control (best), native L2 for others.
- **L3** pluggable proof; **CCIP** as a strong default (RMN, institutional, the track), fast messaging where UX demands, optimistic where cheap.
- **L4** netting + CCTP for residuals.

Symmetric, non-custodial on both sides, proof-pluggable, and honest about the caveats.

---

## Sources
- The Compact: [Uniswap blog](https://blog.uniswap.org/the-compact-v1) · [the-compact repo](https://github.com/Uniswap/the-compact) · [Tribunal repo](https://github.com/Uniswap/Tribunal) · [resource locks vs escrow](https://medium.com/etherspot/chain-abstraction-design-approaches-resource-locks-vs-escrow-contracts-etherspot-arcana-x-41c04531a99a)
- Across / optimistic: [intents architecture](https://docs.across.to/concepts/intents-architecture-in-across) · [Across Prime](https://www.paradigm.xyz/2025/05/across-prime)
- Fusion+ / HTLC: [1inch help](https://help.1inch.com/en/articles/9842591-what-is-1inch-fusion-and-how-does-it-work) · [cross-chain-swap repo](https://github.com/1inch/cross-chain-swap)
- Messaging trust models: [Eco security models](https://eco.com/support/en/articles/14799181-cross-chain-messaging-security-models) · [Spark comparison](https://www.spark.money/tools/cross-chain-messaging-comparison)
- CCTP v2: [Circle launch](https://www.circle.com/pressroom/circle-launches-next-evolution-of-cctp-to-enable-fast-cross-chain-settlement-for-crypto-capital-markets)
- Netting: [Everclear × Across V4](https://www.everclear.org/blog/everclear-powers-rebalancing-for-risk-labs-relayer-in-across-v4) · [solver netting](https://eco.com/support/en/articles/12160293-what-is-solver-netting-how-intents-skip-bridge-hops)
