# Cross-chain implementation and operations

Solvent's cross-chain backend is deliberately three processes: one normal Solvent service owns the
origin chain, one owns the destination chain, and a keyless proxy joins their quotes and advances a
durable saga. A chain service is the only process allowed to read its Aqua book, reserve its local
capital, simulate calls, or submit transactions. The proxy has only authenticated HTTP clients, a
SQLite saga database, and (for routed repayment) a Circle Iris client.

This boundary is more than deployment convenience. A single process with two wallets would blur
which database is authoritative after a crash or reorg. Here, each local database owns its quotes,
prepare tokens, ledger holds, staged calldata, and walletkit handles. The proxy owns only the joined
quote and cross-chain progress.

## End-to-end lifecycle

1. The proxy asks both services for a typed leg quote concurrently. Each service checks its
   configured chain, prices against its own book, fingerprints every authoritative term, and stores
   the exact issued quote locally.
2. The proxy rejects wrong-chain, stale, mismatched, zero-amount, or underfunded combinations. A
   CCTP route also reads Circle's current route fee and Fast Transfer allowance; origin USDC output
   must cover destination repayment plus the fee bound.
3. When an order is submitted, each service accepts only the exact quote it previously issued.
   Both create deterministic prepare tokens and atomically reserve their local Aqua sources. The
   proxy commits and then re-reads both tokens before delivery.
4. The destination service simulates and submits the delivery through its own walletkit engine.
   Once confirmed, it posts its local hold. The settlement contract has already recorded the exact
   fill envelope hash, so anyone can dispatch those bytes through the deferred CCIP outbox.
5. CCIP authenticates the router, remote selector, remote outbox, schema version, proof kind, and
   unique message/proof/order IDs at the origin inbox. Until that asynchronous delivery happens,
   origin settlement simulation is harmless and retryable; it cannot cause a second destination
   delivery.
6. The origin service claims the Compact. A direct route records and dispatches a repayment proof
   back to the destination. A routed route atomically swaps the claimed input into USDC and starts a
   CCTP V2 burn.
7. For CCTP, the proxy polls Iris by the confirmed origin transaction. It validates the fixed-size
   message's source/destination domains, destination caller, mint recipient, hook version, and order
   ID before it ever stages the attestation-bearing destination call. This is why routed close
   calldata cannot be supplied at initial order admission.
8. The destination service confirms the direct repayment proof or calls `completeRepayment` with
   the validated Circle message. The contract repays the maker, releases on-chain exposure, sends
   only explicit surplus to the fee recipient, and returns to its pre-call token balance.

Every irreversible command has a deterministic ID derived from the order, owning chain, and command
kind. Replaying an HTTP request, restarting any process, or running duplicate coordinator ticks
therefore inspects or advances the same operation instead of creating another one.

## Why prepare/commit is not a distributed transaction

There is no atomic transaction spanning two chains and three SQLite databases. Solvent instead uses
a small saga. `prepare` creates an expiring local hold; `commit` makes that hold non-expiring; and
`release` compensates a partial admission only before destination execution. The proxy persists each
token as soon as it receives it. If the second prepare or commit fails, it asks both owners to
release what is still safely releasable. Once delivery may have happened, failures enter
`needs_reconcile`: the coordinator retries from recorded evidence and never frees repayment capacity
merely because a network call timed out.

The important distinction is between uncertainty and failure. A timeout does not prove that a
transaction was not submitted, a CCIP message was not delivered, or a CCTP attestation was not
issued. The owning service's durable handle or authenticated inbox is re-read before another action
is taken.

## Proof and attestation safety

CCIP is used only as an authenticated proof transport. Settlement records evidence synchronously;
dispatch is permissionless and deferred, so a router outage or fee movement does not roll back an
otherwise valid fill. The chain-local service decodes the staged `dispatch` call and refreshes
`IRouterClient.getFee` immediately before simulation/submission. The outbox accepts the exact native
fee, sends no tokens, retains no fee balance, and binds one immutable payload hash to each order.

CCTP is a repayment rail, not a fill proof. The Circle response's message and attestation have
redacting `Debug` behavior and never enter logs or proxy saga JSON. Only the destination chain's
private step store temporarily holds the final calldata required for submission. The destination
contract independently repeats the complete CCTP header, burn-body, fee, finality, sender,
recipient, and order-hook validation before calling the official message transmitter.

## Running the three services

Copy `solvent.example.toml` twice and configure a distinct RPC, chain ID, database, wallet state,
and `[crosschain]` private bind for each chain. Run the normal `solvent` binary once per file with
separate signing keys. Copy `solvent-proxy.example.toml`, point it at both private listeners, and run
`solvent-proxy`. All processes must receive the same `SOLVENT_INTERNAL_TOKEN`; production ingress
must additionally restrict the private listeners and terminate mTLS at the service gateway.

The proxy exposes only:

- `POST /v1/cross-chain/quote`;
- `POST /v1/cross-chain/orders`;
- `GET /v1/cross-chain/orders/{id}`;
- `GET /v1/openapi.json` and `/healthz`.

The browser never receives an operator key. `@solvent/sdk/cross-chain` prepares Compact deposits and
typed signatures, talks only to the proxy, and polls durable order state. `needs_reconcile` is not a
terminal client state: it means the coordinator has post-delivery evidence and is retrying the next
idempotent action.

## Operational checks

- Alert on sagas remaining in one nonterminal state longer than the configured chain finality plus
  proof-rail SLA. Do not automatically release them.
- Keep each chain service's SQLite and walletkit database on the same durable volume and back up the
  proxy database independently.
- Rotate the shared Bearer token at the gateway and process boundary; never place it in TOML.
- Fund chain-service operator accounts for gas. CCIP dispatch fees are refreshed at submission, and
  CCTP fees/allowance are refreshed both during quote and order admission.
- Deploy paired inbox/outbox contracts, initialize each remote address exactly once, then verify all
  selectors, recorders, apps, and settlers against the route manifest before enabling traffic.
- Treat malformed authenticated CCIP/CCTP payloads as security incidents. Transport errors and
  `404`/rate-limit responses are retry conditions.

## Focused regression reruns

Each new regression can be rerun independently from the repository root:

```sh
cd contracts && forge test --offline --match-contract CcipProofTest --match-test test_destinationInboxCanBeInitializedExactlyOnce
cd contracts && forge test --offline --match-contract CcipProofTest --match-test test_fillProofDispatchesAndVerifiesByStoredId
cd contracts && forge test --offline --match-contract CcipProofTest --match-test test_recorderCanBeInitializedExactlyOnce
cd contracts && forge test --offline --match-contract CcipProofTest --match-test test_rejectsSecondProofForOneOrder
cd contracts && forge test --offline --match-contract CcipProofTest --match-test test_rejectsWrongFeeEnvelopeRemoteAndReplay
cd contracts && forge test --offline --match-contract CcipProofTest --match-test test_rejectsWrongRecorderAndConflictingPayload
cd contracts && forge test --offline --match-contract CcipProofTest --match-test test_repaymentProofDispatchesAndVerifiesByStoredId
cd contracts && forge test --offline --match-contract CcipProofTest --match-test test_sourceOutboxCanBeInitializedExactlyOnce
cargo test -p solvent-core crosschain::proxy::tests::aggregate_uses_the_earliest_expiry
cargo test -p solvent-core crosschain::proxy::tests::aggregate_rejects_wrong_chain_identity
cargo test -p solvent-core crosschain::proxy::tests::aggregate_rejects_cctp_origin_shortfall
cargo test -p solvent-core crosschain::proxy::tests::second_service_prepare_failure_compensates_the_first
cargo test -p solvent-core crosschain::proxy::tests::preparation_resumes_after_a_transient_commit_failure
cargo test -p solvent-core crosschain::proxy::tests::coordinator_reaches_complete_with_idempotent_named_steps
cargo test -p solvent-core crosschain::proxy::tests::cctp_completion_is_staged_only_after_the_origin_burn
cargo test -p solvent-core primitives::crosschain::tests::committed_capital_can_be_compensated_until_execution
cargo test -p solvent-core primitives::crosschain::tests::post_delivery_failure_must_enter_reconciliation
cargo test -p solvent-core primitives::ledger::engine::tests::committed_reservation_survives_expiry_and_can_post
cargo test -p solvent-adapters crosschain::cctp::tests::allowance_decimal_never_uses_floating_point
cargo test -p solvent-adapters crosschain::cctp::tests::fractional_basis_points_become_parts_per_million
cargo test -p solvent-adapters crosschain::cctp::tests::completion_rejects_a_message_for_another_order
cargo test -p solvent-adapters crosschain::sqlite::tests::preparation_insert_and_transition_survive_reopen
cargo test -p solvent-adapters crosschain::sqlite::tests::issued_leg_quote_round_trips_exact_terms
cargo test -p solvent-adapters http::crosschain_internal::tests::private_routes_require_the_exact_bearer_credential
cargo test -p solvent-adapters --test ledger_store committed_reservation_remains_in_the_recovery_set
cd sdk && pnpm test -- test/cross-chain/client.test.ts
```

## Deliberate boundaries

This implementation supports a configured EVM origin/destination pair and the existing
`SolventCrossChainOrder` Compact settlement contracts. It does not claim wire compatibility with the
current ERC-7683 draft, choose arbitrary routes across a chain graph, custody user keys, or provide
inventory netting across many fills. Those are separate adapters or policy layers; none is required
to preserve the safety invariants of the implemented direct and CCTP paths.
