# From Form Input to SwapVM: An Invariant Guide

This guide explains where position and swap values originate, how they are
converted into protocol values, which layer validates them, and how to debug a
failure without guessing.

## 1. The central rule: validate in the unit that enforces the limit

The UI displays decimal strings such as `1.25 USDC`. ERC-20, Aqua, SwapVM,
Permit2, and UniswapX operate on integers such as `1_250_000` for a six-decimal
USDC token. A decision made with `Number` can disagree with an on-chain decision
because JavaScript numbers lose integer precision above `2^53 - 1`.

The flow therefore keeps three representations separate:

| Representation | Example | Purpose |
| --- | --- | --- |
| Typed decimal string | `"1.250000"` | Preserve what the user entered and report syntax errors |
| Raw `bigint` | `1_250_000n` | Compare balances and enforce EVM integer domains |
| Display number/string | `$1.25` | Approximate labels only; never transaction eligibility |

`parseTokenAmount` is the conversion boundary. It accepts plain decimal notation,
checks the token's decimal precision, requires a positive value, calls Viem's
`parseUnits`, and enforces the requested integer maximum. Exponent notation,
negative values, empty values, extra fractional digits, zero, and overflow are
rejected before any request is made.

## 2. Position creation from form to chain

### Pair and price

The selected pair carries checksummed, nonzero, distinct token addresses and
token decimals. The live pair midpoint remains the strategy anchor. Chart period
selection only changes the historical series shown to the user; it does not
alter the submitted midpoint or range.

Token orientation matters. Reversing the pair inverts the displayed price and
changes which token is base and quote. The strategy allocator receives the same
orientation and token decimals as the strategy builder, so the displayed reserve
pair and encoded program agree.

### Reserve allocation

Solvent does not maintain a separate approximation of SwapVM curve math. It
delegates allocation to the pinned `@1inch/swap-vm-sdk` primitives:

- Full-range XYC uses the SDK's spot-price reserve calculator.
- Concentrated liquidity uses `PriceRange.computeFixedAllocation` and
  `PriceRange.computeMaxAllocation`.
- Pegged liquidity uses `PeggedSwapCalculator.computeFixedAllocation`.

The allocator exposes four operations: derive a pair from a fixed base reserve,
derive it from a fixed quote reserve, verify that a pair matches the curve, and
find the largest matching pair within both raw wallet balances. This makes Max
and 50% obey the same token order, decimal normalization, and rounding rules as
the encoded SwapVM strategy.

### Strategy construction

Before invoking SwapVM, the SDK checks:

1. Both token addresses are valid, nonzero, and distinct.
2. Both decimal counts are integers in the ERC-20 metadata domain.
3. Concentrated prices are positive and unequal.
4. Pegged reserves are positive and the linear width is inside SwapVM's domain.
5. Fees are integer basis points below 100%.
6. Salt fits `uint64`.
7. Maker is a valid nonzero address.

The builder then produces the SwapVM program, the ABI-encoded Aqua order, and
the strategy hash from one immutable strategy definition.

### Preview and approval

The position client validates the final reserve map before contacting the
backend. It must contain exactly two distinct nonzero token addresses, and every
amount must be positive and fit `uint248`, Aqua's stored reserve width. It also
requires the connected/submitting maker to match the maker encoded in the Aqua
order.

The backend repeats this validation. This repetition is intentional: the SDK is
the developer-facing contract, while the backend is the security boundary for
old or custom clients.

Preview then answers whether the strategy already exists, whether each token
needs approval, and whether current balances are sufficient. An approval is
simulated, sent, confirmed, and followed by an allowance reread before the ship
transaction proceeds.

### Final ship preflight

Immediately before broadcasting, the wallet adapter performs `eth_call` with
the exact sender, target, calldata, and native value that will be sent. A
deterministic contract failure stops here and no transaction hash is created.
After broadcast, the transaction receipt decides success.

## 3. Swap flow from input to settlement

### Quote request

The quote service waits for the settled form input, parses it into raw units,
and rejects invalid precision before calling the HTTP adapter. The backend also
rejects a zero address, identical input/output tokens, and a zero amount before
the router runs.

The router, waterfall solver, candidate pricing, wallet caps, gas handling, and
reserve system are unchanged by this work.

### Quote identity

A quote is a snapshot, not a reusable price label. It carries:

- token-in address;
- token-out address;
- exact raw input amount;
- raw output amount;
- expiry.

Before building an order, the adapter compares all identity fields with the
current form. Editing the amount or reversing the pair invalidates the old
quote. An expired quote cannot be submitted.

### Slippage and minimum output

The form percentage is converted to whole basis points first. Valid values are
`0..=9,999`. The minimum output is calculated with integer arithmetic from the
raw quote output. Both quoted output and minimum output must remain positive and
fit `uint256`.

### UniswapX order

The SDK validates the deployment and signed terms before using the UniswapX
builder: positive safe chain ID, nonzero reactor/Permit2/cosigner/swapper
addresses, distinct nonzero tokens, positive `uint256` amounts, a supported
nonce, and a future deadline.

Approvals can take time, so deadline validation occurs again before signing and
before every HTTP submission. An ambiguous network failure may reuse identical
signed bytes, but only while their deadline remains valid.

### Backend cosigning

The server first rejects a `chainId` different from its configured deployment.
It decodes and verifies the swapper signature, then accepts only the order shape
implemented by the current route normalizer:

- configured reactor;
- nonzero swapper;
- no additional validation contract or validation payload;
- one positive fixed input;
- exactly one positive fixed output;
- distinct input and output tokens;
- output recipient equal to the swapper;
- deadline long enough for the server's decay window.

Only after these checks does the server cosign and pass the order into the
existing route, reserve, persistence, simulation, and fill flow.

## 4. Deterministic failures and live-state failures

A deterministic failure follows directly from submitted values: an oversized
salt, a `uint248` overflow, duplicate tokens, a 100% fee, an expired deadline,
or an unsupported order shape. These are now rejected before wallet interaction
or server signing.

A live-state failure depends on state that can move after validation: another
transaction consumes allowance, a balance changes, the quote expires, or a
token behaves differently at the current block. Approval checks and final
`eth_call` catch these whenever the failure is visible before broadcast. No
client can make the simulation-to-mining interval atomic without changing the
on-chain transaction architecture.

## 5. Debugging sequence

When position creation fails, inspect boundaries in this order:

1. Read `InputValidationError.field`, `code`, and message.
2. Compare typed values with raw `bigint` reserves and token decimals.
3. Confirm `allocator.matches` for the displayed reserve pair.
4. Confirm the preview payload has exactly two positive `uint248` values.
5. Check approval simulation, receipt, and the allowance reread.
6. Check the final ship `eth_call`; if it succeeds but mining fails, inspect the
   intervening state change and receipt revert data.

When a swap fails, inspect:

1. Parsed raw input and token precision.
2. Quote identity and expiry against the current form.
3. Slippage basis points and raw minimum output.
4. Order token addresses, amounts, nonce, and deadline.
5. Backend chain ID and cosigner shape validation.
6. Route/reservation result, then execution simulation and receipt.

This order follows the actual call graph and separates invalid input from stale
state and routing availability.

## 6. Focused regression commands

The full gates are authoritative. These commands isolate the new regression
areas when debugging:

```sh
pnpm --dir sdk vitest run test/validation.test.ts
pnpm --dir sdk vitest run test/construction/allocation.test.ts
pnpm --dir sdk vitest run test/construction/strategy.test.ts
pnpm --dir sdk vitest run test/positions/client.test.ts
pnpm --dir sdk vitest run test/orders/validation.test.ts
pnpm --dir sdk vitest run test/swap/client.test.ts
pnpm --dir sdk vitest run test/wallet/approval.test.ts

pnpm --dir fe vitest run src/adapters/http/swap.test.ts
pnpm --dir fe vitest run src/services/swap.test.tsx
pnpm --dir fe vitest run src/lib/create-position.test.ts

cargo test -p solvent-core maker::service::tests::preview_rejects_amounts_aqua_cannot_store
cargo test -p solvent-adapters http::tests::swap_quote_rejects_zero_amount_and_same_token
cargo test -p solvent-adapters http::tests::swap_rejects_an_order_for_another_chain_before_decoding
cargo test -p solvent-adapters ingest::uniswapx::cosigner::tests::rejects_every_order_shape_the_router_does_not_support
```

For a single Vitest case, append `-t "<complete test name>"` to the corresponding
file command. The release routing stress properties live in
`crates/core/tests/routing_properties.rs` and run individually with:

```sh
cargo test -p solvent-core --release --test routing_properties <test_name> -- --ignored --exact
```
