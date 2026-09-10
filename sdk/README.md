# `@solvent/sdk`

A client-side, **non-custodial** TypeScript SDK for [Solvent](../README.md). It builds strategies, position calldata, and taker orders; reads the backend API; and executes wallet operations through clients supplied by the caller. Private keys and wallet connection remain with the host application.

```bash
pnpm add @solvent/sdk
```

## Architecture — hexagonal, applied where it pays

Pure builders are separate from HTTP and on-chain I/O. Consumers can import individual subpaths:

| Module | Role | I/O |
| --- | --- | --- |
| `construction/` | Strategy program, hash, and order | None |
| `positions/` | Approve, ship, dock, and push calldata | None |
| `client/` | Typed backend API | Injected HTTP transport |
| `orders/` | Build a typed Permit2 order | None |
| `swap/` | Connected swap client and per-intent execution | Injected API and viem clients |

## Repeated maker positions

Use a different salt to ship multiple positions with the same maker, curve, range, fee, and
initial token amounts:

```ts
import { createSolventClient } from "@solvent/sdk/client";
import { Strategy } from "@solvent/sdk/construction";

const api = createSolventClient({ baseUrl: "https://api.example.com" });
const strategy = Strategy.inRange({ base, quote, mid: "3000", halfWidthPct: 8 }).fee(5);
const { taker_credential: takerCredential } = await api.config();
const first = strategy.build(maker, takerCredential);
const second = strategy.salt(1n).build(maker, takerCredential);
```

`salt` reuses SwapVM's `withSalt` instruction, which changes the strategy hash while preserving
the pricing instructions. It accepts an unsigned 64-bit bigint; zero keeps the unsalted program.
The same maker, configuration, and salt reproduce the same hash. `fee` and `salt` return new
builders and compose in either order. Ship both orders with the same token amounts to give
each copy the same initial inventory.

Every strategy starts with SwapVM's `onlyTakerTokenBalanceNonZero` instruction for the deployed
filler credential returned by `GET /config`. This keeps direct public calls from moving the curve
outside Solvent's protected fill and rebate path.

Aqua hashes are immutable, including after a position is docked. A repeat seed checks
`rawBalances.tokensCount`: `0` is unused, `255` is docked, and other values are active. Skip
docked salts and choose an unused salt for a replacement; an existing active copy needs no
new mint, approval, or ship.

## Swapping

Create a client once for the connected wallet. The wallet client must have its chain configured; the SDK also checks the live network before approving or signing. Each intent represents one payment authorization:

```ts
import { createSolventClient } from "@solvent/sdk/client";
import { createSwapClient } from "@solvent/sdk/swap";

// publicClient and walletClient are viem clients supplied by the host.
const api = createSolventClient({ baseUrl: "http://localhost:8080" });
const swaps = createSwapClient({ api, publicClient, walletClient });
const intent = swaps.createIntent({
  swapper: walletAddress,
  tokenIn,
  tokenOut,
  amountIn: 100n * 10n ** 18n,
  minAmountOut: minimumFromQuote,
  deadline: Math.floor(Date.now() / 1000) + 600,
});

const submitted = await intent.submit();
const trade = await api.tradeDetail(submitted.trade_id);
```

`createIntent` snapshots the terms without performing I/O. `submit` fetches deployment settings, checks balance and allowance, confirms any required exact-amount approval, signs the order, and submits it. Concurrent calls share one operation. After an uncertain HTTP failure, retry **the same intent object's `submit()`**; it reuses the signed order so the resolver can deduplicate it. A successful submission response is cached. `SwapDeclinedError` means the resolver explicitly declined that intent; creating a fresh intent is an explicit new attempt.

Submission is an acknowledgment, not confirmation. Read `api.tradeDetail(trade_id)` to follow settlement. Intent state is in memory; applications that need reload recovery must persist and reconcile their own workflow.

`swaps.tokenAccount({ token, owner, spender })` returns `{ balance, allowance }` in bigint base units. The wallet adapter binds dependencies once and keeps account checks, approval simulation, receipt verification, and signing private. A sufficient allowance is reused; a partial allowance is reset before increasing it. Local signers and injected wallets both work without React dependencies. React applications can supply Wagmi's `useClient` and `useConnectorClient` results.

The separate `orders` subpath retains `buildSwapOrder(venue, terms)` for callers that only need unsigned encoding. It uses upstream UniswapX encoding and generates an unordered nonce with Web Crypto when one is not supplied. Display formatting and slippage policy belong to the application.

Clients use **functional factories** with private bound dependencies; API wire types are
**generated from the backend's OpenAPI document** so they cannot drift from the server; failures
are **thrown typed errors**. Packaging is dual ESM/CJS with `sideEffects: false` for full
tree-shaking, and each module publishes its own subpath export (`@solvent/sdk/construction`, …).

## Development

```bash
just sdk-setup   # one-time: install deps
just sdk-gate    # the task gate: typecheck + build + test
```
