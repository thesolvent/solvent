# @solvent/scripts — devnet harness

Bring up a devnet, point the server at it, seed maker liquidity, and check the read API is healthy.

Requires Node **22.18+ (22.x)** or **24.2+** for native TypeScript execution and `import.meta.main`.

Host ports: **8545** chain · **5100** explorer · **8080** Solvent server · **8081** faucet.

## Bring it up

```sh
# 1. Chain + explorer, then the one-shot deploy of the stack and the Core-6 test tokens.
#    The deploy is idempotent: it skips when contracts/deployments/solvent-devnet.json exists.
docker compose -f devnet/docker-compose.yml up -d anvil-1 explorer
docker compose -f devnet/docker-compose.yml up seed

# 2. Generate solvent.toml, the token list, and the signing env from the deploy manifest.
pnpm --dir scripts bootstrap

# 3. Place Multicall3 + Permit2 at their canonical addresses and verify the signing domain.
forge build --root contracts   # once, for the artifact
pnpm --dir scripts etch

# 4. Server.
set -a; . ./devnet/generated/env.sh; set +a
cargo run --bin solvent

# 5. Eight pools, 24 confirmed sample trades, then the read-API health check.
pnpm --dir scripts starter
```

Add the faucet with `docker compose -f devnet/docker-compose.yml up -d faucet` (it builds the Rust
image, so the first run is slow).

To run the chain without Docker, substitute step 1 with the flags the compose file uses — the router
exceeds EIP-170 and finality must advance for finality-anchored confirmation to settle:

```sh
anvil --host 127.0.0.1 --port 8545 --chain-id 31337 --block-time 1 \
      --disable-code-size-limit --slots-in-an-epoch 1 \
      --state devnet/generated/anvil.json --state-interval 5
cd contracts && forge script script/DeployDevnet.s.sol:DeployDevnet --broadcast \
  --rpc-url http://127.0.0.1:8545 \
  --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
```

## Scripts

| Script | What it does |
|---|---|
| `bootstrap` | Manifest → `solvent.toml`, `devnet/generated/tokens.json`, `devnet/generated/env.sh`. Re-run after every deploy. |
| `etch` | Installs Multicall3 and Permit2 runtime code and verifies the Permit2 signing domain on devnet. |
| `seed` | Mints, approves Aqua, and ships eight token pairs. An existing strategy hash is skipped; a changed market mid can create a new strategy on the same pair. |
| `trades` | Adds 24 sample trades: three per seeded pair, covering both directions, sized at roughly $1,000–$1,500 each. Uses the SDK approval/signing flow and verifies every settlement receipt. Each run adds a new batch. |
| `starter` | Runs `seed`, `trades`, then `smoke`. Requires the deployed, configured chain and running API from steps 1–4. |
| `smoke` | Calls the read API through the SDK client and prints a status matrix. Exits non-zero on any failure. |

## Notes

- **The seed is the real write path**: it builds programs with the same `@solvent/sdk` construction
  and positions modules a maker client uses, so a break here is a genuine break.
- The deploy manifest is bind-mounted to `contracts/deployments/` so the host can generate config
  from it; the chain itself persists in the `anvil-state` volume.
- `src/lib/node-runtime.ts` loads the SDK's published CommonJS entry points through `createRequire`
  because upstream ESM assumes a bundler. It also loads viem's client constructors from CommonJS:
  SDK receipt retries rely on matching error constructors. Pure viem helpers can use normal imports.
- Seed/trade modules connect only when invoked as the CLI entry point; importing them performs no
  network calls. Their stages receive the verified devnet connection explicitly.
- Generated config, the token list, the signing env, the database, and the manifest are untracked.
- Permit2 uses the same checked-in runtime fixture as the Rust integration harness. `etch` verifies its domain for chain 31337 before signing tests run.

## Review data

The eight pairs are WETH/USDC, WBTC/USDC, LINK/USDC, DAI/USDC, WETH/DAI, WBTC/DAI,
LINK/DAI, and USDT/USDC. Pair definitions are shared by both scripts in `src/seed/pairs.ts`.
Seeding preserves prior positions and trades. The extra pairs use the existing Core-6 tokens.

The trade generator uses public Anvil account #8, separate from the maker accounts and personal
wallets. It funds gas if needed and mints dev tokens. Both write scripts verify the chain ID,
deployment addresses, and API token metadata against the manifest before writing.

`devnet/generated/review-trades.json` records the batch's trade IDs, status, amounts, and transaction
links, including any submitted trade still awaiting completion if a run fails. The script stops on
a failed/declined trade, a 90-second settlement timeout, or a reverted receipt. A transport retry
reuses the same SDK intent and authorization; rerunning the command starts a new batch. Generated
data stays local and is not included in Git pushes.
