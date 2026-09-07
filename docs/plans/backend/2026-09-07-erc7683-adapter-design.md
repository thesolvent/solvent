# P2 — second protocol adapter (ERC-7683, same-chain) — design

> **Status:** design of record, awaiting review. **Target:** ETHOnline 2026 (submit Sept 13).
> Scope: **hermetic, same-chain ERC-7683 v1** — a second intent protocol, with a genuinely different
> order ABI and fill entrypoint, normalizing into the same `Intent` and reusing routing, ledger,
> execution and recapture **unchanged**. Cross-chain 7683, third-party settlers, and the v2 resolver
> redesign are out of scope (§11). Backend phases B0–B5 + Tier-0 recapture are prerequisites.
>
> **Phase numbering is open.** `SPEC.md` §13 calls this "P2 — second adapter"; the backend B-series
> already spends B6 on reconcile and B7 on backtest. Named P2 here pending a decision.

---

## 1. Why this exists

"Protocol-agnostic" is a headline claim of this project ([`docs/ARCHITECTURE.md`](../../ARCHITECTURE.md)),
and today it rests on a single adapter. The ports were built for it — `OrderFeed`, `Normalizer`,
`FillBuilder`, and an `IngestPipeline` that already dispatches by `ProtocolId` and fans in N feeds —
but an abstraction with one implementation is an assertion, not a proof.

A second protocol is worth building only if it is **genuinely different**, not a re-skin. ERC-7683
qualifies on both axes that matter:

- **A different order envelope.** Opaque `orderData` behind an EIP-712 `orderDataType`, versus
  UniswapX's concrete `V2DutchOrder`.
- **A different fill shape, and this is the interesting part.** UniswapX hands the filler a
  **callback** (`reactorCallback`) in which the swapper's input has already arrived. ERC-7683's
  `IDestinationSettler.fill(orderId, originData, fillerData)` hands the filler **nothing** — deliver
  the output first, be repaid by settlement afterwards.

That second difference is a direct attack on the zero-inventory thesis, which is exactly why it is
worth doing. §5 resolves it without holding inventory and without asking the settler for favors.

---

## 2. What ERC-7683 is, and the version fork

ERC-7683 standardizes the **solver-facing surface** of an intent protocol: how an order is expressed,
how a solver reads it, and what function it calls to fill. It is not a bridge and not a settlement
system. It deliberately leaves settlement/proof, the order feed, solver selection, and the encoding
of `orderData` outside its scope.

Two order envelopes wrap an opaque payload:

```solidity
struct GaslessCrossChainOrder {          struct OnchainCrossChainOrder {
    address originSettler;                   uint32  fillDeadline;
    address user;                            bytes32 orderDataType;
    uint256 nonce;                           bytes   orderData;
    uint256 originChainId;               }
    uint32  openDeadline;
    uint32  fillDeadline;
    bytes32 orderDataType;
    bytes   orderData;
}
```

`resolve*` (view) turns either into a common `ResolvedCrossChainOrder` carrying `maxSpent`,
`minReceived` and `fillInstructions`, so a solver can price an order without a protocol-specific
decoder. `IOriginSettler` exposes `open`/`openFor`/`resolve`/`resolveFor` + an `Open` event;
`IDestinationSettler` exposes only `fill`.

**The fork, as of 2026-09.** The text at [eips.ethereum.org/EIPS/eip-7683](https://eips.ethereum.org/EIPS/eip-7683)
was rewritten on **2026-05-13** ("Update ERC-7683: Redesign around resolvers") into a different model
entirely — a generic `IResolver` returning steps/variables/payments, with ERC-7930 interoperable
addresses — and is still **Draft**. Everything above is **v1**, whose last content change was
2025-01-08 and which is what protocols actually deployed. Across's own interface file now carries the
header: *"This ERC-7683 interface is from a previous version of the standard. A new version of
ERC-7683 is being developed and will replace this interface."*

**Decision: build against v1.** v1 is what exists on chain; v2 has no deployments and no settled
semantics. This is recorded here so a future reader who opens the EIP and finds an unrelated spec
knows the choice was deliberate.

---

## 3. Prior art

| System | 7683 role | Settlement | We borrow / differ |
|---|---|---|---|
| **Across** ([docs](https://docs.across.to/developer-quickstart/erc-7683-in-production), [contracts](https://github.com/across-protocol/contracts/blob/master/contracts/interfaces/ERC7683.sol)) | reference implementation of the v1 order struct; `AcrossOriginSettler` forwards into `SpokePool.depositV3()`, `SpokePool` implements `IDestinationSettler` | UMA optimistic — the relayer is repaid hours after filling | We take the **interface**, not the settlement. Optimistic repayment requires holding a position, which is the opposite of our thesis (§11) |
| **UniswapX** | 7683-compatible from launch; our existing adapter talks to the reactor natively | atomic, same-chain | Our existing zero-inventory pattern comes from here; §5 reproduces it without a reactor callback |
| **Eco** ([deep dive](https://eco.com/support/en/articles/14796366-erc-7683-cross-chain-intents-standard-deep-dive)) | 7683 endpoints over its own prover | prover-gated | Confirms settlement is per-protocol, hence out of the standard |
| **erc7683.org** ([spec](https://www.erc7683.org/spec)) | the v1 reference text | — | Source of the structs we vendor |

**Finding that shaped the design:** there is no standalone, reusable reference settler pair. Across's
`IDestinationSettler` lives inside `SpokePool.sol` with its HubPool/UMA dependency graph, so it cannot
be dropped onto a devnet. Hence §4's decision to write a minimal generic settler.

---

## 4. Scope

**In:** a minimal, generic, same-chain ERC-7683 v1 settler deployed on the devnet; a separate
Aqua-sourcing filler for it; the Rust adapter family; and an E2E in which **both protocols contend
for one maker balance** through one pipeline.

**Out:** cross-chain 7683 (§11), third-party/mainnet settlers (§11), the v2 resolver model (§2), and
any change to routing, ledger, execution, recapture or the registry.

**The honest caveat, stated up front:** we author the settler we fill against. This proves the
*adapter seam* — that a genuinely different order ABI and fill entrypoint reuse the whole core — and
does **not** prove live-market reach. §12 keeps this visible rather than buried.

---

## 5. The mechanism — zero inventory without a reactor callback

### 5.1 The problem

`IDestinationSettler.fill` gives the filler nothing. To deliver the output we must first buy it from
a maker, and to buy it we need the user's input, which is escrowed until the fill is proven. Circular.

Three escapes exist: front the capital (kills the thesis), have our settler call the filler back
(works, but bakes a favor into a settler that is supposed to be generic), or find a flash primitive
elsewhere. The third is available and costs nothing new.

### 5.2 The primitive we reuse

`SwapVM._settle` orders its transfers by a taker flag, and our filler already uses the second branch:

```solidity
if (takerTraits.isFirstTransferFromTaker()) { _transferIn(...);  _transferOut(...); }
else                                        { _transferOut(...); _transferIn(...);  }
```

So the **maker pays the taker first**. And `_transferIn` fires `ITakerCallbacks.preTransferInCallback`
on the taker *before* pulling the taker's payment. Between those two points the filler is holding the
maker's output having paid nothing — a flash swap, provided by the router we already use.

`TakerTraitsLib.Args` already exposes `hasPreTransferInCallback` and `preTransferInCallbackData`; the
existing [`UniswapXAquaFiller._takerTraits`](../../../contracts/src/UniswapXAquaFiller.sol) sets both
off. The 7683 filler turns them on. **No new library, no hand-rolled flash loan.**

### 5.3 The atomic sequence (single leg)

```
fill(settler, orderId, originData, sources)      [onlyOwner, nonReentrant]
  snapshot balances of every touched token
  router.swap(makerOrder, USDC→ETH, exactOut, hasPreTransferInCallback: true)
    _transferOut   AQUA.pull(maker) ─────────────→ filler holds ETH, has paid nothing
    _transferIn    preTransferInCallback ──┐
                                           │  approve(settler, outputAmount)
                                           │  settler.fill(orderId, originData, fillerData)
                                           │      ETH:  filler ──→ user
                                           │      USDC: escrow ──→ filler      (same-chain settlement)
                                           └─ return
                   transferFrom(filler, USDC) + AQUA.push ──→ maker paid
  assert every touched token non-decreasing
```

One transaction. No inventory at any point. The spread stays in the filler and is swept like today.

### 5.4 Multi-leg: nested, bounded

Only one swap can host the callback, and at that moment the filler holds only that leg's output —
but `settler.fill()` needs the whole amount. Leg *i*'s callback therefore launches leg *i+1*; the
innermost callback holds the full output and calls `settler.fill()`; the stack unwinds paying makers
outward. The remaining plan rides in `preTransferInCallbackData`, so the filler keeps **no contract
state** between levels. SwapVM's reentrancy guard is keyed per `orderHash`, so distinct maker orders
nest cleanly — this relies on the routing invariant that a plan carries **at most one leg per
strategy**, which `RoutePlan` already satisfies (one leg per `StrategyKey`). Two legs against the same
maker order would deadlock on that guard; the `FillBuilder` rejects such a plan rather than emitting it.

Bounded at `MAX_LEGS = 4`: recursion depth and gas stay predictable, and the `FillBuilder` rejects
longer plans off-chain rather than reverting on-chain.

### 5.5 Safety layers

The three layers of the UniswapX filler carry over, with the middle one re-sourced:

1. **Per-leg `amountInMaximum`** as the SwapVM taker threshold — caps what any maker is paid.
2. **The settler's own `outputAmount` check** replaces reactor-derived approvals: an under-sourcing
   plan cannot satisfy `fill()`.
3. **The balance-snapshot profitability guard** — every touched token ends at least where it started.
   This remains the backstop that makes (2) airtight: if under-sourcing were covered by the filler's
   own accrued spread, the guard catches the decrease and reverts.

Plus: `onlyOwner` on the entrypoint, `ReentrancyGuardTransient`, and a transient `_routerInFlight`
authenticating the callback — the same pattern as today's `_reactorInFlight`.

The nesting does not trip the filler's own reentrancy guard: recursion re-enters through
`preTransferInCallback` and a private helper, never through the guarded `fill` entrypoint, and
`settler.fill()` reaches the filler only as an ERC-20 `transferFrom`, never as a callback.

---

## 6. The settler

`SameChainSettler.sol` implements **both** `IOriginSettler` and `IDestinationSettler`, because on one
chain origin and destination are the same contract. It is generic: it grants the filler nothing that
it would not grant any other filler.

**Our order type** (`orderDataType = keccak256("SolventOrder(...)")`), deliberately *not* a Dutch
decay, so the second protocol is economically a different animal:

```solidity
struct SolventOrder {
    address inputToken;   uint256 inputAmount;
    address outputToken;  uint256 outputAmount;   // fixed price
    address recipient;
    address exclusiveFiller;                      // address(0) = open
    uint32  exclusivityEnds;
}
```

- **`openFor(order, signature, originFillerData)`** — verifies the user's EIP-712 signature and
  escrows their input in one call via Permit2 **`permitWitnessTransferFrom`**, with the order as the
  witness. That is the same primitive UniswapX uses, already vendored at
  `contracts/lib/UniswapX/lib/permit2/`, and it supplies signature verification, `openDeadline`, and
  nonce-replay protection for free. Emits `Open(orderId, resolvedOrder)`.
- **`fill(orderId, originData, fillerData)`** — requires the escrow open and unfilled, enforces
  `fillDeadline` and the exclusivity window against `msg.sender`, pulls `outputAmount` from the filler
  to the recipient, then **releases the escrowed input to the filler in the same call**. On one chain,
  proof-of-fill *is* the fill; the standard leaves settlement to the implementer.
- **`resolve` / `resolveFor`** — pure decode into `ResolvedCrossChainOrder`. This is what makes it a
  real 7683 settler rather than a bespoke escrow, and it is what a third-party solver would call.

`orderId` = the EIP-712 hash of the `GaslessCrossChainOrder`. The Rust codec reproduces it, and a
fixture test pins Rust against Solidity (§10).

---

## 7. Architecture — what changes

```
NEW    contracts/src/interfaces/ERC7683.sol      v1 structs + interfaces, vendored from the spec
NEW    contracts/src/SameChainSettler.sol        §6
NEW    contracts/src/Erc7683AquaFiller.sol       §5
NEW    crates/adapters/src/ingest/erc7683/       codec · normalizer · fill · builder · feed
EDIT   ProtocolId                                += Erc7683
EDIT   FillBuilderError                          += TooManyLegs
EDIT   contracts/script/DeployDevnet.s.sol       deploy settler + filler into the manifest
UNTOUCHED  routing · ledger · execution · recapture · registry · IngestPipeline · Intent · FillTx
           UniswapXAquaFiller
```

That last block is the deliverable. Specifically, nothing in the following needs to change:

- **`IngestPipeline`** already holds `BTreeMap<ProtocolId, Arc<dyn Normalizer>>` and already fans in
  `Vec<Arc<dyn OrderFeed>>`. A second protocol is one map entry and one more feed.
- **`Intent`** already carries `settler`, `raw`, `signature`, `origin_chain`, and an `Exclusivity`
  whose semantics ("only this filler until `ends_at`, matched against the settling contract's
  `msg.sender`") hold identically on both protocols.
- **`AmountCurve::scalar`** already expresses a fixed-price amount, so no curve code is added.
- **`FillTx`** already carries `filler`, so a 7683 intent targets `Erc7683AquaFiller` and a UniswapX
  intent targets `UniswapXAquaFiller` with no type change.

### 7.1 The one new piece of plumbing

Nothing dispatches `ProtocolId → FillBuilder` today; the E2E tests select `UniswapXFillBuilder`
directly. Per YAGNI this does **not** become a port or a service. The two-protocol E2E gets a
`BTreeMap<ProtocolId, Arc<dyn FillBuilder>>` lookup at the call site, mirroring the normalizer map,
and that map moves into the composition root when it lands (blocked, see §12).

---

## 8. Data flow (end to end)

```
SelfHostedFeed(7683)  ─┐
                       ├─→ IngestPipeline ─→ Normalizer[Erc7683] ─→ Intent ─┐
SelfHostedFeed(UniX)  ─┘   (unchanged)      Normalizer[UniswapXV2]          │
                                                                            ▼
                                       route() ─→ RoutePlan ─→ LedgerService.reserve()
                                                                            │
                                             FillBuilder[protocol].build()  ▼
                                                                        FillTx
                                                                            │
                                       ExecutionService: sim → submit → confirm
                                                                            │
                                             AquaSettlementReader → post actual pulls
                                                                            │
                                                        recapture (unchanged)
```

Both protocols share every box after `Intent`. The reservation ledger sees one pool of maker budget
regardless of which protocol asked, which is what makes §10's contention test meaningful.

---

## 9. Errors & observability

No new public error type. `FillBuilderError` gains `TooManyLegs` (the `MAX_LEGS` rejection); the
normalizer reuses `NormalizeError::Decode(ProtocolId::Erc7683)`. Everything downstream already
surfaces as `SolventError`.

Instrumentation follows the existing shim and level discipline — the 7683 path adds no new secret
material (the swapper's signature is already carried opaquely as `Intent::signature` and never
logged), so the redaction allow-lists are unchanged.

---

## 10. Testing

**Foundry — settler.** open→fill→escrow-release happy path; double-fill reverts; expired
`fillDeadline` reverts; exclusivity enforced against a non-exclusive filler and released after
`exclusivityEnds`; replayed Permit2 nonce reverts; `resolve` round-trips an order.

**Foundry — filler.** Single-leg fill against source-deployed Aqua/SwapVM + the settler; 2-leg and
4-leg nested; `MAX_LEGS + 1` reverts; unauthorized `preTransferInCallback` reverts; under-sourced
output reverts; the profitability guard reverts when a leg overpays.

**Rust unit.** `orderId` fixture test pinning the Rust codec against the Solidity hash, generated by
a forge script in the pattern of `GenV2OrderFixture.s.sol` — this is the test that actually catches
breakage. Normalizer rejects a malformed payload. `FillBuilder` maps legs and rejects `> MAX_LEGS`.

**E2E — the one that matters.** `e2e_two_protocol.rs`: one `IngestPipeline`, both feeds, a UniswapX
order and a 7683 order that both want **the same maker balance**. Assert the ledger grants one and
declines the other, the granted one fills on chain, and the declined one fills once the first settles.

Per house rules, no tests are added for struct init, the `sol!` derives, or the deploy script.

---

## 11. YAGNI — deliberate non-goals

- **No cross-chain 7683.** `IntentOutput` has no per-output chain id, and cross-chain settlement needs
  an in-flight-capital concept the ledger does not have. Adding either "for later" is exactly what the
  house rules forbid.
- **No third-party settler integration.** Across-style optimistic repayment means holding a position
  for hours; that is a different product, not a bigger adapter.
- **No `ProtocolAdapter` super-port.** The three existing ports already carve the protocol seam
  correctly. Bundling them would be an abstraction with two implementations and no caller asking.
- **No v2 resolver support.** Nothing is deployed against it.
- **No self-signed order production.** The settler implements `open`/`resolve` because
  `IOriginSettler` mandates them and a spec-mandated method is a deliberate API surface, not dead
  code — but the feed, the normalizer and every test drive the **gasless** `openFor`/`resolveFor`
  path, which is the flow a resolver actually sees. No `OrderSpec` variant produces on-chain orders.

---

## 12. Honest limits & risks

- **We author the settler we fill.** This proves the adapter seam, not live-market reach (§4). It is
  disclosed rather than implied.
- **Deliberate DRY deviation.** `Erc7683AquaFiller` duplicates ~150 lines of sourcing machinery
  (`SourceSwap`, the sourcing loop, `_snapshot`/`_assertNonDecreasing`, `_takerTraits`) rather than
  sharing an extracted `AquaSourcing` base. This was chosen knowingly to avoid touching a working,
  tested contract days before the deadline; it contradicts the "extract at the second use" house rule.
  **Follow-up: extract the base after the deadline**, at which point any guard fix stops needing to be
  made twice. Logged in `KNOWN_LIMITATIONS.md`.
- **`MAX_LEGS = 4`.** Plans wider than four makers are rejected off-chain on the 7683 path. UniswapX
  is unaffected. Raising it is a constant plus a gas measurement.
- **Nested callbacks are the sharpest code in the phase.** They move real money and unwind in reverse.
  This is the task most likely to consume time and the one that most needs adversarial tests.
- **No binary.** `crates/app/src/main.rs` is still `fn main() {}` — the composition root is blocked
  work tracked in `KNOWN_LIMITATIONS.md` L10. "Both protocols in one engine" is therefore demonstrated
  by an E2E test, not by a running binary. Stated so no demo claims more than it does.
- **Settlement is trivial because the chain is one.** The design does not solve cross-chain
  settlement and does not claim to; `docs/CROSS_CHAIN_SETTLEMENT_PRIMER.md` remains the forward look.

---

## 13. Decisions locked in the design brainstorm

1. **v1, not v2** of the standard (§2).
2. **Hermetic, self-authored, same-chain settler** rather than a third-party or forked one (§3, §4).
3. **Zero inventory preserved via `preTransferInCallback`**, not via a settler-side callback and not
   via a working-capital buffer (§5.2).
4. **Nested-bounded multi-leg**, `MAX_LEGS = 4`, over single-leg-only — so the 7683 path exercises the
   same multi-maker water-fill routing as UniswapX (§5.4).
5. **Separate filler contract with accepted duplication**, over an extracted shared base (§12).
6. **Fixed-price + exclusivity order type**, not a Dutch decay, so the second protocol is genuinely
   different while reusing `AmountCurve::scalar` (§6).
7. **Permit2 `permitWitnessTransferFrom`** for signature + escrow + nonce + deadline in one call (§6).
8. **No `ProtocolId → FillBuilder` port**; a call-site map until the composition root exists (§7.1).

---

## 14. Open questions

- **Phase numbering** — P2 (product) vs a new B-number; `KNOWN_LIMITATIONS.md` already spends B6/B7.
- **Does the settler belong in `contracts/src/` or a `contracts/src/testing/` namespace?** It is a
  real contract, but it exists to be filled against rather than to be the product. Leaning
  `contracts/src/` with the caveat documented, since the devnet deploys it for real.
- **Whether `RESOLVER_FLOW_CATALOG.md` Flow B's claim that every 7683 protocol becomes "reachable for
  free" should be softened now.** It overstates: the fill entrypoint and resolved-order shape are
  shared, but the feed, the `orderData` decoder, and the repayment risk model stay per-protocol.
