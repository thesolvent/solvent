# Prior work

Solvent is submitted to **ETHOnline 2026** (Sept 4–16). This repository was created on
**2026-09-05** by importing an existing Solvent codebase. The commits below are dated to
that import — they record when the code landed *here*, not when it was written.

## What pre-dates the hackathon

Everything in the import series (the commits from `chore: repository scaffolding` through
`docs: CHANGELOG`) was written between **2026-08-28 and 2026-09-03**, before the hackathon
started, in a separate repository. That covers:

- **`contracts/`** — `UniswapXAquaFiller` and its test suite, `DevToken`, the devnet deploy script
- **`crates/`** — the Rust workspace: registry, routing, ledger, ingest, execution, app, faucet
- **`devnet/`** — the Docker Compose devnet stack
- **`SPEC.md`**, **`docs/`**, **`CHANGELOG.md`** — the spec, architecture, and research write-ups

Third-party code is vendored or referenced, not authored here: the UniswapX reactor
(`contracts/lib/UniswapX`, v2.1.0) and the audited 1inch Aqua + SwapVM contracts that Solvent
composes as a taker.

## What is hackathon work

Everything committed from **2026-09-05** onward, after the import series. Track it here as it
lands so the new-vs-reused split stays legible to reviewers.

| Date | Change | Where |
| ---- | ------ | ----- |
| _(add entries as work lands)_ | | |
