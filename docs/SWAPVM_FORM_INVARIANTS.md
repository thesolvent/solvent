# SwapVM Form Invariants

## Goal

Prevent deterministic position-creation and swap reverts by enforcing the
protocol's value domains before a wallet approval, signature, or transaction is
requested. The SDK owns exact protocol validation, the frontend preserves and
displays those failures, and the backend independently validates every trusted
boundary.

This work does not change quote optimization, waterfall routing, reserve
lifecycle, curve execution, settlement, or on-chain contract behavior.

## Why backend changes are required

Frontend validation gives immediate feedback but is not a trust boundary. An
older frontend, a third-party SDK consumer, or a manually constructed HTTP
request can bypass it. Three backend checks are therefore required:

1. Position preview validates the exact two-token shape and Aqua's `uint248`
   reserve domain before calculating approvals.
2. Swap submission rejects an order for another chain before decoding or
   cosigning it.
3. The server cosigner accepts only the fixed one-input/one-output UniswapX V2
   order shape the current router understands.

These are additive boundary guards. No backend pricing or routing algorithm had
to change.

## Invariant table

| Boundary | Required invariant | Failure prevented |
| --- | --- | --- |
| Position pair | Two valid, nonzero, distinct token addresses | Ambiguous or invalid Aqua reserve map |
| Position maker | Transaction sender matches the maker encoded in the Aqua order | Shipping another maker's strategy definition |
| Position reserve | `0 < amount <= 2^248 - 1` | `SafeCast.toUint248` failure in `Aqua.ship` |
| Strategy fee | Integer basis points in `0..=9,999` | Exact-out division failure at a 100% fee |
| Strategy salt | Unsigned 64-bit integer | Strategy program encoding failure |
| Concentrated range | Positive, distinct, representable bounds | Invalid square-root price construction |
| Pegged range | Positive reserves and SwapVM width at or below `5000e27` | Pegged program construction failure |
| Swap amount | Plain positive decimal within token precision and `uint256` | Truncation, zero orders, or `parseUnits` failure |
| Slippage | Integer basis points in `0..=9,999` and positive minimum output | Zero-output or effective 100% slippage order |
| Quote identity | Same token pair and exact raw input used for submission | Signing terms different from the displayed quote |
| Signed order | Valid venue, distinct tokens, positive `uint256` amounts, future deadline | Permit2 or builder rejection |
| Retry | Deadline is rechecked before every submission | Reusing an expired cached signature |
| Server order | Configured reactor, one flat input/output, recipient is swapper, no hook | Cosigning a shape the router misinterprets |
| Cosigner window | Deadline covers the configured decay interval | Reactor rejection of an invalid decay window |
| Wallet transaction | Final account, target, value, and calldata pass `eth_call` | Broadcasting a transaction already known to revert |

## Ordered implementation and rationale

### 1. Exact SDK validation primitives — complete

- [x] Add `InputValidationError` with a field, stable code, and readable message.
- [x] Validate addresses, unsigned EVM integer domains, token decimals, decimal
  amounts, fees, and salts in one module.
- [x] Export only validation helpers used by the SDK or frontend.

One definition prevents the form, SDK builders, and wallet path from accepting
different numeric domains.

### 2. Strategy and position construction — complete

- [x] Validate token metadata and concentrated price bounds before constructing
  SwapVM price objects.
- [x] Validate pegged reserves and linear width before constructing the program.
- [x] Restrict fees to integer `0..=9,999` basis points and salts to `uint64`.
- [x] Require exactly two distinct positive `uint248` reserves before preview.
- [x] Require the submitting maker to match the maker encoded in the strategy.

These checks stop deterministic strategy and Aqua encoding failures before any
wallet interaction.

### 3. Exact wallet balances and position form — complete

- [x] Add raw base-unit balances to pair responses while keeping display values.
- [x] Carry raw balances as `bigint` through the frontend.
- [x] Calculate Max, 50%, balance coverage, and submission eligibility in base
  units.
- [x] Cap custom fees at 99.99%, preserve invalid text, and show the exact error.
- [x] Delegate XYC, concentrated, and pegged allocation math to the pinned
  SwapVM SDK.

This removes JavaScript floating-point values from decisions enforced by ERC-20
and Aqua integer arithmetic.

### 4. Wallet preflight — complete

- [x] Simulate the final transaction with `eth_call` immediately before send.
- [x] Stop before broadcasting when simulation fails.
- [x] Keep the mined receipt as the authority after a transaction is broadcast.

This catches allowance, balance, and live-state failures that form validation
cannot predict.

### 5. Swap input and quote binding — complete

- [x] Parse the input using token decimals before requesting a quote.
- [x] Reject invalid syntax, excess precision, zero, and `uint256` overflow.
- [x] Convert slippage to basis points first and restrict it to `0..=9,999`.
- [x] Reject zero quoted output or zero minimum output.
- [x] Bind quotes to token-in, token-out, exact raw input, and expiry.
- [x] Preserve invalid typed text and present the precise validation failure.

The submitted order now uses the exact terms the user saw and authorized.

### 6. Order construction and retry freshness — complete

- [x] Validate venue addresses and chain ID before invoking the UniswapX builder.
- [x] Validate tokens, amounts, nonce, recipient, and deadline.
- [x] Recheck expiry before signing and before every submission, including cached
  retries.

Cached authorization cannot bypass the freshness checks used by the first
attempt.

### 7. Backend trust boundaries — complete

- [x] Reject malformed Aqua reserve sets during position preview.
- [x] Reject zero-address, same-token, and zero-amount quote requests.
- [x] Reject wrong-chain swap submissions before decoding.
- [x] Validate the reactor, swapper, tokens, fixed amounts, output count,
  recipient, absent validation hook, and deadline window before cosigning.
- [x] Leave quote math, waterfall routing, reservations, simulation, and execution
  unchanged.

Server approvals and signatures no longer depend on browser validation.

### 8. Verification — complete

- [x] Exercise exact integer boundaries, amount precision, fees, salts, ranges,
  quote identity, deadlines, and nonces in SDK tests.
- [x] Verify invalid frontend values never reach quote, preview, signing, or
  submission calls.
- [x] Verify a failed wallet simulation never broadcasts.
- [x] Exercise malformed previews, wrong-chain requests, and unsupported cosigner
  order shapes in Rust.
- [x] Verify XYC, concentrated, and pegged allocation in both token orientations
  with mixed decimals.
- [x] Run frontend, SDK, default-feature, no-default-feature, all-feature, and
  ignored routing correctness stress suites.

### Verification record

| Gate | Result |
| --- | --- |
| Frontend format, lint, typecheck, tests, build | Passed; 27 files and 142 tests |
| SDK typecheck, tests, build, generated-client check | Passed; 11 files and 121 tests; one environment-gated live settlement test skipped |
| OpenAPI snapshot and generated TypeScript client | Passed and current |
| Rust format | Passed |
| Rust Clippy, default features | Passed with warnings denied |
| Rust Clippy, no default features | Passed with warnings denied |
| Rust Clippy, all features | Passed with warnings denied |
| Rust tests, default features | Passed, including all backend end-to-end suites |
| Rust tests, no default features | Passed, including all backend end-to-end suites |
| Rust tests, all features | Passed; 333 active tests across unit, integration, differential, and end-to-end suites |
| Ignored routing correctness stress properties | Passed 8 of 8 in release mode |

The repository's `just be-gate` alias could not run because `just` is not
installed in the environment. Its Cargo commands were run directly, including
the default, no-default-feature, and all-feature variants.

## Guarantee boundary

The application now prevents value-domain reverts that it can determine before
submission. A transaction can still fail if chain state changes between the
final simulation and mining, a token has nonstandard behavior, or an external
contract changes. The immediate simulation makes that remaining window as small
as the current transaction architecture permits.
