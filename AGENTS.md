# Solvent — working agreement

Rules that override default behavior. Read **Core rules** first; the **Collaboration
workflow** governs how we go from idea to merged PR; the rest are engineering rules for the
code itself. (Backend = the Rust workspace under `crates/`; on-chain = `contracts/`.)

## Core rules (non-negotiable)

- **Frontend design is a hard constraint.** Preserve the original mock/hardcoded layouts,
  dimensions, typography, colors, charts, controls and field placement. Wire real data into
  existing slots. Ask the user before adding, removing or moving a visible field, link, control
  or panel, or changing the visual design. Missing API data is a backend requirement, not
  permission to redesign or remove UI. This applies to existing pages as well as new work.
  Already reviewed pages outside the current task stay unchanged; do not restore them to an
  older mock snapshot. Ask before changing their behavior or navigation.

- **IMPORTANT: never commit until I have reviewed and approved** — code, docs, and plans
  alike. "Consistency" or "it's obvious" is not approval. **Confirm before every `push`.**
  **No `Co-Authored-By` trailer** on commits.
- **IMPORTANT: no `unwrap()`/`expect()` in production code.** Propagate with `?`; allowed
  only in `#[cfg(test)]`, `const`, or a documented genuinely-infallible invariant (the
  `expect` message states it). Panic = programmer bug — no `anyhow` in core/adapters.
- **IMPORTANT: redaction is mandatory on key paths** (see Observability) — no secret ever
  reaches a log, error, span, or `Debug`.
- One task at a time: write the whole task, run the gate, report the **real** output, stop
  **uncommitted**.
- Gate before stopping: `cargo fmt --check` · `cargo clippy --all-targets` (zero warnings) ·
  `cargo test`. Green **with and without** `--no-default-features` (`just be-gate`).
- Every public fallible API returns `SolventError` — never a raw port error or
  `Result<_, String>`.
- Update `CHANGELOG.md` `[Unreleased]` before opening any PR.
- **YAGNI is a core rule, not a nicety** — add a type/field/variant/dep only at its first
  real consumer. Do not pre-build classification, config fields, ports, or abstractions
  "for later" (e.g. no `ErrorKind`/`kind()` until a caller actually branches on it).

## Collaboration workflow

**Per phase (plan → implement → merge):** the backend is built phase-by-phase (B0…B5), each
phase 3–5 tasks, each task **one complete component**.

1. **Design first** — the all-phases architecture spec lives at
   `docs/plans/backend/2026-08-29-backend-design.md` (already reviewed). For anything
   non-trivial, run a **research pass**: survey how leading libraries/services solve it, cite
   sources, let it shape the design.
2. **Plan a phase** — I present the phase's 3–5 tasks **in chat**; you approve each; I write
   the locked plan to `docs/plans/backend/<phase>-plan.md`. Each task names the **library
   primitive** each non-trivial step reuses (decided at plan time, not during coding).
3. **Implement task-by-task** (see below). Branch `feat/backend` off `main` (direct pushes to
   `main` disabled).
4. **Phase-close pass** — after the phase's last task, run a AGENTS.md-standards refactor +
   review over the whole phase (fresh reviewer: correctness **and** house rules); apply cleanups.
5. **Review before commit** — I review the phase **uncommitted**. On approval, **commit the
   phase** and move to the next. `CHANGELOG.md` `[Unreleased]` updated before any PR; **merge
   only when I say so**.

**Per task:**

1. Implement one plan task; write the **entire** task.
2. Run the gate and report the **real** output (never claim green without showing it). For
   each test added, give the exact single-test re-run command
   (`cargo test -p <crate> <path>` / `cargo test -p <crate> --test <bin> <name>`).
3. **Teach** (my learning preference): write a deep, self-contained article on the concepts
   the task touched — the exception to normal terseness; I'm mastering these topics.
4. Stop **uncommitted**. Commit only per the phase-review gate above.

## Architecture — Cargo workspace, hexagonal (design §2)

- `crates/core/primitives/` — value/domain types, organized per domain: `shared/` (cross-cutting
  foundation) + one module per phase (`registry/`, `ledger/`, …). `deps/` — ports. `<slice>/service.rs`
  — the slice's orchestration. **Zero I/O; cannot depend on adapters.**
- `crates/core/deps/<slice>/*.rs` — ports: `#[async_trait]` behind `Arc<dyn Trait>`, **one
  file per port**, each with its own `thiserror` enum `{TraitName}Error`. Ports are derived
  per component in that component's phase — not batch-locked up front.
- `crates/adapters/` — port implementations (RPC/DB/signer/order-feed) + inbound handlers.
  The heavy backtest stack sits behind a `backtest` cargo feature (the live binary never
  compiles revm-for-backtest).
- `crates/app/` — the binary: composition root + worker loops (watcher + decision).
- Prefer **fewer, larger, focused files** over many tiny ones.

## Code style — house rules (only what differs from defaults)

- **Comments say why, not what** — short, minimal; most lines need none. No comment-per-change,
  no dev-process breadcrumbs (task/step numbers, "grows in Phase N", "was/refactored"). **No
  references to the design doc, plan, phases, or task numbers** (`§7.10`, `design §…`, `Phase N`,
  `Bx`, `Lab vector`) — a comment explains the code, not where it came from; put spec cross-refs in
  the docs, not the source. Roadmap/future references are noise. Naming the upstream being ported
  (e.g. `XYCSwap._xycSwapXD`) is fine (it *is* the why). Doc summaries 1–2 sentences.
- **Naming:** accessors drop `get_` (bare noun), writes are domain verbs, predicates are
  `is_`/`supports_`. Fix outliers to match.
- **YAGNI** (see Core rules) — a deliberate *public API surface* (a re-export, a spec-mandated
  method) is not "unused"; its consumer is the downstream caller.
- **Reuse before hand-rolling** — if a library fn, a built-in/default, or a solid crate does
  the job, use it and delete what the library provides (`alloy`/`amm-core`/`walletkit`/`serde`/
  `sqlx`/`rust_decimal`/`std`). Decide the primitive at **plan-writing** time.
- **Named returns, not positional tuples** — a fn returning >1 typed value returns a named
  struct; call sites read `.field`, never `.0`/`.1`. (Exceptions: same-type pairs, `(K, V)`.)
- Prefer `match`/combinators over nested `if/else`; extract shared logic at the second use
  (DRY). Prefer `parking_lot` / `ArcSwap` locks (no poisoning). `#[non_exhaustive]` on returned
  structs/enums so they can grow without breaking callers.
- **Encode invariants in the type system** where possible — make the unsafe path
  *unrepresentable* (e.g. wrong-ID-to-wrong-fn is a compile error), not merely discouraged.

## Observability & errors

- One public error type (`SolventError`); per-port `{Trait}Error`s map in via `From`. Add
  classification (`kind()` → `Retryable`/`Terminal`/`NeedsReconcile`) and `remediation()`
  **only when a caller needs to branch on it** (YAGNI).
- Instrument via the shim (`use crate::obs::{info, warn, error, debug};`), never `tracing::`
  directly; spans via `#[cfg_attr(feature = "tracing", tracing::instrument(...))]`. Never
  install a subscriber or depend on `opentelemetry` — the host owns that.
- Correlate by `IntentId` (one span per intent). Levels: ERROR = terminal caller-facing ·
  WARN = recoverable (bump/reorg/failover/retry) · INFO = sparse milestones (~1/intent) ·
  DEBUG = mechanics · TRACE = raw. Keep INFO sparse.
- **Redaction is mandatory on key paths.** Any fn touching a key, a tx to sign, or signed
  bytes is `#[instrument(skip_all, fields(<safe allow-list>))]` — allow-list, never deny-list.
  No secret ever becomes a span/event field; secret-bearing types get a redacting `Debug`.
  Keep the redaction test green.

## Tests — every test earns its place

- **No tests** for config parsing, serde derives, struct init, route registration, trivial glue.
- **Test only** logic that can regress: orchestration, error/edge paths, SQL correctness,
  non-trivial computation (pricing, water-fill, ledger invariants).
- In-memory fakes for core unit tests; a live dependency (env-gated) for adapter integration
  tests. Prefer **hermetic** harnesses (embedded anvil, cheat-codes, source-deployed Aqua/
  SwapVM) over external endpoints; when asserting real-chain values, pin a fork block and
  compute expected values (`cast`), never approximate.

## After each task — checklist

- [ ] Workspace layout respected; ports one-per-file with `{TraitName}Error`.
- [ ] YAGNI (no unused types/fields/deps, nothing pre-built); reuse check (nothing hand-rolled
      that a library provides).
- [ ] Comments why-not-what; naming matches house style.
- [ ] Only regression-worthy tests added; each with its single-test re-run command reported.
- [ ] Public failures return `SolventError`; key paths instrumented `skip_all`, no secrets in
      telemetry; green with and without `--no-default-features`.
- [ ] Gate run, real output reported. Learning article written. Left uncommitted; commit only
      at the phase-review gate.
- [ ] At phase close: phase-close review pass done; `CHANGELOG.md` `[Unreleased]` updated before the PR.
