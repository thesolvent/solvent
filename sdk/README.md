# `@solvent/sdk`

A client-side, **non-custodial** TypeScript SDK for [Solvent](../README.md). It builds
market-maker strategies and the calldata to open, top-up, and close positions, and it reads
the backend API — all from the browser or a script. It **never holds keys and never sends a
transaction**: the position builders return unsigned `{ to, data, value }` calldata for the
maker's own wallet to sign and broadcast.

```bash
pnpm add @solvent/sdk
```

## Architecture — hexagonal, applied where it pays

The backend is strict hexagonal (pure `core` · `deps` ports · `adapters` · `app`). A client SDK
does not need that shape wholesale: ~90% of it is pure computation with a single I/O boundary.
So the SDK keeps the *spirit* — a pure core isolated from I/O, dependency inversion at the one
seam that varies — without ceremony:

| Module | Role | I/O | Depends on |
| --- | --- | --- | --- |
| `construction/` | build a strategy program + hash + order | none (pure) | `@1inch/swap-vm-sdk` |
| `positions/` | encode approve / ship / dock / push calldata | none (pure) | `@1inch/aqua-sdk`, `viem` |
| `client/` | typed reads + writes over the HTTP API | **the only I/O** | a `Transport` port |

The two pure modules need no ports — there is nothing to invert a dependency against (the same
reason Uniswap's SDK and `@1inch/aqua-sdk` are plain functions). The one real seam is the HTTP
**`Transport`** — a `fetch`-shaped function injected into `createSolventClient(...)`, defaulting
to the platform `fetch`. That is the whole hexagon: pure core, one port, one adapter.

Further decisions: the client is a **functional factory** (not a class); its wire types are
**generated from the backend's OpenAPI document** so they cannot drift from the server; failures
are **thrown typed errors**. Packaging is dual ESM/CJS with `sideEffects: false` for full
tree-shaking, and each module publishes its own subpath export (`@solvent/sdk/construction`, …).

## Status

Scaffold in place (build · typecheck · test toolchain). Modules land across M5:
`construction` (T2) · `positions` (T3) · `client` + OpenAPI codegen (T4).

## Development

```bash
just sdk-setup   # one-time: install deps
just sdk-gate    # the task gate: typecheck + build + test
```
