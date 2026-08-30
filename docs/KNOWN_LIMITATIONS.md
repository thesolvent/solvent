# Known limitations & deferred work

Correctness gaps found by adversarial corner-case review that are **intentionally deferred** to a
later phase (each is documented at its code site too). None is an over-commitment or fund-loss risk:
every one degrades safely (under-report / miss a fill / self-heal on restart). Fixed findings are not
listed here — they live in the code and its tests.

The review method, for reuse: per component, trace the code against ① a tombstone/inactive/terminal
state read as if live, ② idempotency/replay safety, ③ external state changing under us, ④ numeric
edges, ⑤ ordering / concurrency / crash-consistency.

---

## L1 — Late-`Shipped` balance loss in the live snapshot
**Component:** registry — `crates/core/src/registry/sync.rs` × `crates/core/src/primitives/registry/snapshot.rs`
**Class:** ⑤ ordering (cross-cycle)

`Snapshot::apply` drops a `Pushed`/`Pulled` to a not-yet-registered strategy (correct per event — you
cannot push to a strategy that has not shipped). The sync applies each newly-inserted event once, then
marks it seen. So if a block's `Shipped` (log 0) is delivered in a **later** sync cycle than that
block's `Pushed`es (RPC partial-block delivery dropping the lowest log index — the same assumption the
`applies_late_arriving_event_below_the_tip` test encodes), those `Pushed`es are applied-and-dropped,
stored, marked seen, and **never re-applied** when the `Shipped` finally lands. The live snapshot then
under-reports that strategy's virtual balance.

- **Blast radius:** the ledger's strategy-virtual budget reads low → it **under-promises** (misses
  fills). Never an over-commit. **Self-heals** on `recover()` / restart (recovery folds the durable
  log in order).
- **Within-cycle** out-of-order delivery is already handled — `sync_once` sorts the inserted batch by
  cursor before folding (test `folds_inserted_in_cursor_order_regardless_of_source_order`). This item
  is only the **cross-cycle** case.
- **Disposition:** repaired by the planned **periodic reconcile (Task M — multicall balance
  reconciliation)**, which re-reads on-chain balances and corrects snapshot drift. A targeted fix
  (re-fold a strategy's stored events when its `Shipped` registers late) is possible but more code for
  a rare, safe-direction, self-healing case.

## L4 — Reorg replacement logs are not yet distinguished
**Component:** registry — `crates/core/src/registry/sync.rs`
**Class:** ⑤ crash/chain-consistency

The sync's in-memory `seen` cache keys on `(block, log_index)`, while the store's unique key is
`(chain, block, block_hash, log_index)`. A reorg's replacement log (same position, new block hash)
would be filtered by the `seen` cache as "already seen" and never reach the store or snapshot live.

- **Blast radius:** stale snapshot across a reorg until restart; the block hash provenance needed to
  detect it is already stored.
- **Disposition:** **Task R (reorg handling)** — detect a stored-vs-fetched block-hash mismatch, prune
  the non-canonical rows, and rebuild the in-memory snapshot. The `seen` cache will then key on (or be
  bypassed by) the block hash.

## L5 — `price()` does not filter inactive strategies
**Component:** registry — `crates/core/src/registry/pricing.rs`
**Class:** ① inactive-as-live

`price()` prices whatever `MakerStrategy` it is handed, including a docked tombstone (which keeps its
old balance). Correctness relies on callers routing only active strategies via
`Snapshot::active_strategies_for_pair`, which they do. (The ledger's `AlloyBudgetSource` was the one
path that read a strategy balance without this filter — now fixed to check `active`.)

- **Blast radius:** none today (no caller prices an inactive strategy).
- **Disposition:** latent inconsistency; documented at the call site. Revisit if a non-routing caller
  ever needs to price directly.

## L6 — Registry ↔ ledger startup ordering & mid-flight virtual drift
**Component:** integration — `crates/core/src/ledger/service.rs` (`recover`) + composition root
**Class:** ③ external-state / ordering

1. **Startup ordering:** `LedgerService::recover()` reads budgets from the current sources, so the
   **registry must be synced/recovered before the ledger recovers**, or a strategy virtual reads zero
   until its next reserve refreshes it. This is a composition-root sequencing requirement (documented
   at `recover`).
2. **Mid-flight drift:** a strategy's virtual can drop between reserve and settlement (another taker
   swaps it down) — the same class as the *tested* wallet-budget drop. Caught at reserve time by the
   JIT firm confirm; a drop *after* reserve is caught by reconcile, with the on-chain `pull` revert as
   the final backstop.

- **Disposition:** (1) enforce in the composition root when the app binary is wired; (2) reconcile
  (Task M) + the on-chain revert already cover the after-reserve drop.

---

# Performance — refactor before production

MVP-simple choices that are correct but do unwanted work / hold heavy state. None is on the 500 ms
quote path (that path is in-memory / lock-free reads); all are on the slower write/sync routes.

## P1 — Full snapshot clone every sync cycle
`RegistrySync::sync_once` does `(*self.snapshot.load()).clone()` then re-publishes — an O(strategies)
deep copy of the whole registry on **every** cycle that inserts an event. Fine at MVP scale, wasteful
as the registry grows. → apply the fold to a `Arc::make_mut` / persistent structure, or batch
publishes, or a copy-on-write snapshot with structural sharing.

## P2 — Per-cycle sort of the inserted batch
`sync_once` sorts the newly-inserted events by cursor each cycle for a robust fold order. Cheap
(batches are small) but strictly redundant when the source already returns sorted logs. → drop it once
the `ChainSource` contract guarantees cursor order, or sort only when out-of-order is detected.

## P3 — `available_snapshot()` rebuilt on every ledger command
`LedgerService::publish` rebuilds the entire `AccountKey → available` map (`available_snapshot`) and
swaps it into `ArcSwap` after every reserve/post/void. O(accounts) per command. → publish incremental
deltas, or shard the snapshot by maker/token.

## P4 — Writer lock held across the durable write
`LedgerService` holds its `tokio::Mutex` across the store `.await`, serializing all writers on the
(local, fast) SQLite write. Correct (single-writer by design) and fine for SQLite, but it caps write
throughput. → group-commit batching, or the command-actor / Disruptor model, when write volume demands.

## P5 — JIT budget confirm is one multicall per wallet account, not batched across sources
`AlloyBudgetSource` issues one Multicall3 round-trip per `WalletBudget` account. A multi-source reserve
therefore makes N round-trips. → batch the whole reserve's confirm into a single multicall (design
calls for "1 batched JIT confirm"). Also: the event-sourced zero-RPC budget cache replaces this on the
quote path entirely (a later phase).
