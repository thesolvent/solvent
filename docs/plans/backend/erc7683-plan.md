# ERC-7683 same-chain adapter — phase plan (locked)

> Design of record: [`2026-09-07-erc7683-adapter-design.md`](2026-09-07-erc7683-adapter-design.md).
> **Branch off `main`** at the phase-commit gate; implement uncommitted until then. **Gate each
> task:** `just be-gate` — `cargo fmt --check` · `cargo clippy --all-targets` (zero warnings) ·
> `cargo test`, green **with and without** `--no-default-features`. Stop uncommitted; commit only at
> the phase review.

Four tasks, **ingest-first**: the adapter that consumes 7683 orders lands before the contracts it
will eventually fill, so the engine can ingest the new protocol on its own. Each task names the
library primitive it reuses (decided here, not during coding).

**Ordering note.** The `Erc7683FillBuilder` is deliberately *not* in Task 1. Its first real consumer
is the filler contract (Task 3); building it earlier would be speculative code, which YAGNI forbids.

---

## Task 1 — ERC-7683 ingest adapter  *(consume-it milestone)*

**Deliver** `crates/adapters/src/ingest/erc7683/` — `codec.rs`, `normalizer.rs`, `builder.rs`,
`feed.rs`, `mod.rs` — plus `ProtocolId::Erc7683`. After this task `IngestPipeline` consumes 7683
orders exactly as it consumes UniswapX ones: same `Intent`, same admission checks, same dedup, both
feeds fanned in. No contract, no fill path.

**Wire format.** The envelope is the standard's `GaslessCrossChainOrder`; `orderData` carries our
`SolventOrder` order type (fixed price + exclusivity window), selected by
`orderDataType == keccak256(SolventOrder's EIP-712 type string)`. `orderId` is the envelope's EIP-712
struct hash — what the Task-2 settler will record.

**Reuses:** alloy `sol!` + **`SolStruct::eip712_hash_struct` / `eip712_encode_type`** — unlike
UniswapX (whose type string flattens `baseInput`, forcing a hand-composed hash), the 7683 envelope is
a plain EIP-712 struct, so alloy derives both the type string and the struct hash and we hand-roll
nothing. `AmountCurve::scalar` for the fixed amounts (no new curve code).
`Exclusivity` as-is. The Permit2 witness digest mirrors
[`uniswapx/builder.rs`](../../../crates/adapters/src/ingest/uniswapx/builder.rs)'s `witness_digest`,
composing the witness type string from `GaslessCrossChainOrder::eip712_encode_type()` rather than a
literal.

**Steps**

- [ ] **1.1** Add `ProtocolId::Erc7683` to
      [`primitives/ingest/intent.rs`](../../../crates/core/src/primitives/ingest/intent.rs); generalize
      the now-wrong `IntentId` doc ("UniswapX order hash" → the source protocol's order hash) in
      [`shared/ids.rs`](../../../crates/core/src/primitives/shared/ids.rs).
- [ ] **1.2** `codec.rs`: `sol!` the two structs; `solvent_order_type() -> B256` and
      `order_id(&GaslessCrossChainOrder) -> B256` over alloy's derived EIP-712.
- [ ] **1.3** Failing test first — `codec::tests::solvent_order_type_string_is_pinned`: assert
      `SolventOrder::eip712_encode_type()` equals the exact expected string. This is the
      cross-language contract with the Task-2 settler; if alloy ever reorders or renames a field the
      settler silently diverges, so it earns its place.
- [ ] **1.4** `normalizer.rs`: `Erc7683Normalizer` → `Intent`. Rejects (as
      `NormalizeError::Decode(ProtocolId::Erc7683)`) a malformed payload, an unrecognized
      `orderDataType`, and an `originChainId` disagreeing with the feed's chain.
- [ ] **1.5** Tests for 1.4 — well-formed order maps every field; wrong `orderDataType` rejected;
      chain mismatch rejected; malformed payload rejected; zero `exclusiveFiller` → `None`.
- [ ] **1.6** `builder.rs`: `SolventOrderSpec` + `SignedOrderBuilder` producing a `RawOrder` whose
      signature is the swapper's Permit2 witness signature over the envelope.
- [ ] **1.7** Test — the signature recovers to the swapper (mirrors
      `both_signatures_recover_to_their_signers`); `v ∈ {27, 28}`.
- [ ] **1.8** `feed.rs`: `SelfHostedFeed` over built specs; test that a streamed order normalizes.
- [ ] **1.9** Test — `IngestPipeline` with **both** normalizers and **both** feeds emits one
      `Intent` per protocol. This is the task's headline: the pipeline is unmodified.
- [ ] **1.10** Run `just be-gate`; report the real output. Update `CHANGELOG.md` `[Unreleased]`.

**Known gap, closed in Task 2.** `order_id` cannot be pinned against Solidity until the settler
exists. Task 2 adds `GenSolventOrderFixture.s.sol` and a `tests/ingest_erc7683.rs` fixture test in
the shape of [`ingest_uniswapx.rs`](../../../crates/adapters/tests/ingest_uniswapx.rs). Until then the
hash is self-consistent but unverified against the chain.

**Re-run:** `cargo test -p solvent-adapters ingest::erc7683` ·
`cargo test -p solvent-core primitives::ingest`.

---

## Task 2 — `SameChainSettler.sol`

**Deliver** `contracts/src/interfaces/ERC7683.sol` (v1 structs + interfaces, vendored from the spec)
and `contracts/src/SameChainSettler.sol` implementing both `IOriginSettler` and `IDestinationSettler`
(design §6), plus `GenSolventOrderFixture.s.sol` and the Rust fixture test that pins Task 1's
`order_id` against the contract.

**Reuses:** Permit2 **`permitWitnessTransferFrom`** (vendored at `contracts/lib/UniswapX/lib/permit2/`)
for signature + escrow + nonce + `openDeadline` in one call; OZ `SafeERC20`,
`ReentrancyGuardTransient`.

**Tests:** open→fill→escrow-release; double-fill reverts; expired `fillDeadline` reverts; exclusivity
enforced then released; replayed Permit2 nonce reverts; `resolve` round-trips.
**Re-run:** `forge test --match-contract SameChainSettler` ·
`cargo test -p solvent-adapters --test ingest_erc7683`.

---

## Task 3 — `Erc7683AquaFiller.sol` + `Erc7683FillBuilder`

**Deliver** the filler (design §5) — `fill(settler, orderId, originData, SourceSwap[])`,
`preTransferInCallback`, nested-bounded chaining at `MAX_LEGS = 4` with the remaining plan carried in
`preTransferInCallbackData` — and, now that it has a consumer, the Rust
`crates/adapters/src/ingest/erc7683/fill.rs` plus `FillBuilderError::TooManyLegs`.

**Reuses:** **`ITakerCallbacks` + `TakerTraitsLib.build{hasPreTransferInCallback,
preTransferInCallbackData}`** (SwapVM pays the taker before taking payment — the flash primitive that
keeps zero-inventory, design §5.2); transient `_routerInFlight` auth mirroring `_reactorInFlight`;
`SafeERC20.forceApprove`.

**Tests:** single-leg fill against source-deployed Aqua/SwapVM + the settler; 2-leg and 4-leg nested;
`MAX_LEGS + 1` rejected off-chain and reverting on-chain; unauthorized callback reverts;
under-sourced output reverts; profitability guard reverts on an overpaying leg.
**Re-run:** `forge test --match-contract Erc7683AquaFiller` ·
`cargo test -p solvent-adapters ingest::erc7683::fill`.

---

## Task 4 — Two-protocol contention E2E + devnet + docs

**Deliver** `crates/adapters/tests/e2e_two_protocol.rs`, the `DeployDevnet.s.sol` additions (settler +
filler into the address manifest), and the doc updates.

**Reuses:** the existing anvil harness (`common::Harness`/`Stack`); `IngestPipeline` unchanged; a
call-site `BTreeMap<ProtocolId, Arc<dyn FillBuilder>>` (design §7.1 — deliberately not a port).

**Headline test:** one pipeline, both feeds, a UniswapX order and a 7683 order contending for **the
same maker balance** → the ledger grants one and declines the other → the granted one fills on chain
→ the declined one fills once the first settles.

**Docs:** `CHANGELOG.md` `[Unreleased]`; a `KNOWN_LIMITATIONS.md` entry for the deliberate filler
duplication (design §12) and the `MAX_LEGS` bound; soften
[`RESOLVER_FLOW_CATALOG.md`](../../RESOLVER_FLOW_CATALOG.md)'s "reachable for free" claim to name what
actually transfers (fill entrypoint + resolved-order shape) versus what stays per-protocol (feed,
`orderData` decoder, repayment model).
**Re-run:** `cargo test -p solvent-adapters --test e2e_two_protocol`.

---

## Phase close

Fresh-reviewer pass over the whole phase (correctness + CLAUDE.md house rules), cleanups applied,
`CHANGELOG.md` `[Unreleased]` current, then review **uncommitted** and commit the phase on approval.
