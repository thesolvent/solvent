# Solvent — flow catalog & component inventory

Assumes you know how Aqua (`ship`/`dock`/`pull`/`push`, virtual balances, `app == msg.sender`) and The Compact (resource locks, allocator, arbiter, Tribunal) work. See the other study docs for those.

---

## 0. The universal skeleton (every flow is this; only the adapter changes)

```
OFF-CHAIN (Rust)                                   ON-CHAIN (Solidity)
ingest ─► normalize ─► price ─► reserve ─► execute ──► UniversalResolver (an Aqua app)
  │         │           │         │          │            │
 order    Normalized  maker's   TTL lease  sign+send   1. JIT AQUA.pull(maker, sh, outToken, amt, →user/settler)
 feed     Intent      SwapVM    on shared  fill tx     2. protocol-specific settle (get the input)
          (port)      quote     balance                3. AQUA.push(maker, sh, inToken, amt)  ← repay maker
                                                        4. keep spread
```

**Only three things vary per protocol:**
1. **Order format** → the adapter normalizes it to `NormalizedIntent`.
2. **The fill entrypoint** (Reactor callback vs Settler.fill vs SpokePool.fillV3Relay vs Tribunal.fill).
3. **The L2 settlement** = how the filler is repaid the *input* (atomic same-chain vs Compact lock vs UMA optimistic vs hashlock).

Everything else — Aqua liquidity (L1), pricing, reservation, maker reconciliation — is shared. That shared core is the product; the adapters are thin.

**Profit model (all flows):** resolver spread = (input value received) − (value pushed to maker at the maker's SwapVM quote) − gas − (cross-chain: settlement + rebalance cost). The maker earns their quoted spread; the resolver captures the gap between the external order's price and the maker's tighter quote.

---

## Flow A — UniswapX order (same-chain, atomic) ✅ primary

**Setup:** maker `ship()`s a two-token quoted position (e.g. USDC/ETH SwapVM strategy) to `UniversalResolver` on chain X.

1. User signs a UniswapX order (Permit2, Dutch decay): sell 1000 USDC → ETH.
2. Off-chain: adapter reads the order feed → `NormalizedIntent`; pricing engine calls the maker's SwapVM `quote()` for ETH out vs 1000 USDC in; if the order's current decay price ≥ maker quote + costs → profitable → reserve a lease on the maker's ETH.
3. Execute: resolver calls `Reactor.executeWithCallback(order, callbackData)`.
4. Reactor pulls the user's 1000 USDC (Permit2) into itself, then invokes `UniversalResolver.reactorCallback(...)`.
5. **In the callback:** `AQUA.pull(maker, sh, ETH, ethAmt, →Reactor)` — maker's ETH goes to the reactor (which forwards to the user). Approve reactor for the output.
6. Reactor sends ETH → user, and sends the 1000 USDC → resolver (the filler).
7. Resolver `AQUA.push(maker, sh, USDC, makerUsdc)` — repays the maker their quoted USDC. Maker position: ETH↓, USDC↑ (a swap at their quote). Resolver keeps `1000 − makerUsdc` spread.

**All steps 4–7 are one atomic tx.** If the maker's ETH can't be pulled (real balance drained), the pull reverts → the whole fill reverts → resolver just loses the auction, no harm. **True zero-inventory.**

**Code:** `reactorCallback` implementation + the pull/push reconciliation. (Uniswap track.)

---

## Flow B — ERC-7683 order (same-chain) ✅ primary

Identical skeleton to Flow A; different interface.

1. User signs a `GaslessCrossChainOrder` (or `OnchainCrossChainOrder`) whose `orderData` encodes input/output for a *same-chain* route.
2. Adapter decodes `orderData` (per its `orderDataType`) → `NormalizedIntent`. Price + reserve as in A.
3. The origin settler `open()`/`openFor()` escrows the user's input on chain X.
4. Resolver calls the **DestinationSettler** `fill(orderId, originData, fillerData)` — sources the output via `AQUA.pull(maker, sh, outToken, amt, →user)`.
5. Settlement releases the escrowed input to the resolver (same-chain → immediate); resolver `AQUA.push` repays the maker; keeps spread.

**Code:** a 7683 adapter (decode `orderData`) + a `fill()` handler that pulls from Aqua. Proves "protocol-agnostic" with a genuinely different order/settler ABI than UniswapX.

---

## Flow C — a 1inch SwapVM position feeding the resolver (double-duty pricing)

This is the "our own liquidity" flow — two readings, both true:

**C1 — the maker's SwapVM program *is* the on-chain quote.** The maker ships a SwapVM strategy (the exact programs from the swapvm-lab: XYC, pegged, limit, gated, etc.) to `UniversalResolver`. To price any external order (A/B/D/E), the resolver runs that program via `quote()` — no off-chain price trust; the maker's curve is the price floor. The resolver reuses our SwapVM quoting stack verbatim.

**C2 — same wallet capital, dual-shipped = double-duty.** A maker already running a 1inch position on `AquaSwapVMRouter` ships a *second* virtual balance (same tokens, same wallet) to `UniversalResolver`. Aqua allows this (virtual balances are over-committable; `ship` is an allowance, not custody). Both positions are backed by the one real wallet balance; whichever `pull()`s first wins, the other reverts if the wallet is drained. So the maker's 1inch quoting capital *simultaneously* backs intent fills — the double-duty claim, realized as two ships of one balance.

**Code:** embed/call SwapVM `quote()` as the pricing oracle (reuse `swapvm-lab`); the multi-app dual-ship is a maker-side action, no new contract.

---

## Flow D — Compact cross-chain (the flow we control) ✅ the cross-chain flagship

User: 1000 USDC on chain A → ~0.3 ETH on chain B. Resolver is the filler.

```
ORIGIN A                                        DESTINATION B
1. User deposits 1000 USDC → The Compact
   (ERC-6909, non-custodial); signs a compact +
   mandate: "pay ≤1000 USDC to whoever delivers
   ≥0.3 ETH on B before T; arbiter=Tribunal".
   Allocator co-signs (anti-over-commit).
2. Intent broadcast → resolvers auction.
                                                3. Winner (us) calls Tribunal.fill() on B,
                                                   delivering 0.3 ETH to user via
                                                   AQUA.pull(makerB, sh, ETH, 0.3, →user).
                                                   makerB ETH↓.
                                                4. Tribunal verifies (expiry/chainId/amount),
                                                   emits directive/proof toward origin arbiter.
5. Proof arrives (L3: CCIP / optimistic / pull).
   Origin arbiter submits claim → allocator OKs
   nonce → 1000 USDC releases to resolver on A.
6. Reconcile makerB: resolver now holds USDC on A
   but owes makerB on B. Repay via netting
   (opposite B→A flow) or CCTP-bridge the residual.
```

**Atomicity:** user-atomic (funds move only on proof, else refund after reset). Filler bears deliver-then-claim risk, *minimized* because the user's funds are pre-locked. **The maker (makerB) is repaid on B via the netting/rebalance layer, not instantly** → this is where cross-chain zero-inventory degrades to "minimal-inventory + netting" (§ research doc).

**Code:** `Tribunal.fill()` caller with Aqua pull; an L2 Compact-claim adapter; an L3 proof adapter (CCIP receiver etc.); the L4 netting + CCTP rebalancer; the reservation engine's **in-flight** capital state.

---

## Flow E — Across order (cross-chain, external L2 = UMA optimistic) ⚠️ weak fit

1. User calls `SpokePool.depositV3` on origin A: input + desired output/recipient on B + relayer fee.
2. Relayers race. Resolver (as relayer) calls `SpokePool.fillV3Relay` on B, delivering output via `AQUA.pull(makerB, …, →user)`. makerB↓.
3. Resolver's reimbursement is **deferred**: Across bundles fills, UMA optimistic oracle validates over a challenge window, then the HubPool repays the relayer (on a repayment chain).
4. During that window the resolver's claim is unrealized and makerB is unpaid → **capital locked in-flight for minutes**. Reconcile makerB via netting after reimbursement.

**Why weak:** Across fuses L2 into a *slow* optimistic reimbursement, so Aqua's JIT benefit is diluted (capital tied up through the window) and bond/slashing makes over-commitment dangerous → **pessimistic leasing required** for this adapter. Include it to prove "protocol-agnostic," not as a capital-efficiency showcase.

**Code:** an Across adapter (watch `V3FundsDeposited`, call `fillV3Relay`) + reimbursement tracker + pessimistic-lease policy.

---

## Component inventory (everything to code)

Legend: 🟢 reuse from `swapvm-lab`/minis · 🟡 adapt · 🔴 new.

### On-chain (Solidity, thin — the chain is the atomic guard)
| Component | Role | Status |
|---|---|---|
| `UniversalResolver` (Aqua app) | holds JIT `pull`/`push` + maker reconciliation; deployed per chain | 🔴 |
| SwapVM quote integration | run maker programs to price fills (Flow C1) | 🟢 reuse `swapvm-lab` |
| UniswapX `reactorCallback` | Flow A fill hook | 🔴 |
| ERC-7683 `fill()` handler | Flow B/D destination fill | 🔴 |
| Across `fillV3Relay` caller | Flow E | 🔴 (thin) |
| `Tribunal.fill()` caller | Flow D destination | 🔴 (thin) |
| L2 Compact-claim adapter | origin claim (Flow D) | 🔴 |
| L3 proof adapter(s) (CCIP receiver, …) | deliver fill-proof to origin arbiter | 🔴 |
| MockERC20 / test scaffolding | fork + local tests | 🟢 |

### Off-chain (Rust, hexagonal — where the intelligence lives)
| Component | Role | Status |
|---|---|---|
| `NormalizedIntent` + `SettlementRequirements` domain types | common representation | 🔴 |
| `ProtocolAdapter` port + adapters (UniswapX, 7683, Across, Compact/own feed) | ingest + normalize | 🟡 (rfq-quoter mini is a seed) |
| Pricing/profitability engine | call SwapVM `quote()`, subtract gas/fees, decide | 🟡 (risk-calculator mini) |
| Inventory manager + **reservation ledger** (single-writer, TTL leases, in-flight state) | the core safety engine (§ research doc §4) | 🔴 |
| Risk engine (per-chain/token caps, exposure, adverse-selection) | bounds | 🔴 |
| Execution engine (build/sign/submit per protocol) | fire the fill | 🟡 (walletkit patterns) |
| Settlement state machine (event-sourced FSM per order) | seen→priced→reserved→committed→delivered→claimed→reconciled/failed | 🔴 |
| `CrossChainSettlement` port + adapters (Compact, UMA/Across, CCIP proof) | L2/L3 | 🔴 |
| Netting engine + CCTP rebalancer (L4) | close residual cross-chain drift | 🔴 |
| Reconciler | read on-chain Aqua/wallet/Compact → correct ledger drift | 🟡 (indexer mini) |
| Indexer / subgraph (The Graph) | Aqua ship/pull/push + fills + claims → dashboard + reconciler feed | 🟡 (indexer mini) |

### Maker-facing surface
| Component | Role | Status |
|---|---|---|
| Utilization dashboard (Graph-backed) | "your capital did the work of N×" | 🔴 (Graph track) |
| Onboarding / ship-dock UI (Privy embedded wallet) | makers supply liquidity | 🔴 (Privy track) |
| EIP-712 clear-signing (Ledger) | maker `ship()` / resolver ops | 🟡 (Ledger track) |

**MVP cut (same-chain):** `UniversalResolver` + SwapVM quote + `reactorCallback` (A) + reservation ledger + reconciler + subgraph + dashboard. Flows B/D/E and the L3/L4 stack are post-MVP.

---

## Extensibility — what else can plug into this flow

The resolver is a **liquidity backend + settlement adapter**. *Any* order flow whose filler role is "permissionlessly deliver output, then get repaid via some settlement primitive" is just another `ProtocolAdapter`. The moat (shared non-custodial Aqua liquidity + reservation engine) is reused unchanged. Ranked by fit:

**Tier 1 — atomic same-chain (cheapest adapters, best fit):**
- **CoW Protocol** — solvers settle batch auctions providing liquidity; adapter = CoW settlement solver interface. Large flow.
- **0x RFQ / Bebop / Hashflow** — RFQ MM networks; we quote via Aqua SwapVM and sign RFQ responses. Natural (Aqua *is* an MM inventory).
- **Any ERC-7683 same-chain settler** — once the 7683 adapter exists, every 7683-compatible protocol is reachable for free.

**Tier 2 — cross-chain via a settlement primitive we already support:**
- **1inch Fusion / Fusion+** — in-domain (hashlock escrow); high value but KYC-gated. Aqua-backed Fusion+ resolver.
- **DeBridge DLN** — cross-chain limit/intent orders; solvers "fulfill" then claim. Adapter = DLN take/claim.
- **Eco Routes, LI.FI intents, Catalyst** — intent/resource-lock settlement; slot under the Compact/L3 stack.
- **Relay Protocol** — relayer-based cross-chain; adapter like Across but different reimbursement.

**Tier 3 — adjacent, same primitive different purpose:**
- **Liquidations backstop** — Aqua liquidity fills liquidation auctions (Aave/Morpho) JIT.
- **Chain-abstraction / EIP-7702 batched intents** — a resolver that fills bundled cross-chain user ops.
- **On-chain limit-order books** — fill resting orders from Aqua inventory.

**The generalization to state in the pitch:** *we didn't build "a UniswapX filler" or "an Across relayer" — we built a **non-custodial liquidity + reservation substrate** onto which any deliver-then-settle protocol drops as a thin adapter. Each new protocol is ~one adapter + one settlement mapping; the capital layer never changes.*
