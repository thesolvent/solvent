# @solvent/scripts — devnet harness

Bring up a devnet, point the server at it, seed maker liquidity, and check the read API is healthy.

Host ports: **8545** chain · **5100** explorer · **8080** Solvent server · **8081** faucet.

## Bring it up

```sh
# 1. Chain + explorer, then the one-shot deploy of the stack and the Core-6 test tokens.
#    The deploy is idempotent: it skips when contracts/deployments/solvent-devnet.json exists.
docker compose -f devnet/docker-compose.yml up -d anvil-1 explorer
docker compose -f devnet/docker-compose.yml up seed

# 2. Generate solvent.toml, the token list, and the signing env from the deploy manifest.
pnpm --dir scripts bootstrap

# 3. Place Multicall3 at its canonical address (a fresh chain hosts no code there).
forge build --root contracts   # once, for the artifact
pnpm --dir scripts etch

# 4. Server.
set -a; . ./devnet/generated/env.sh; set +a
cargo run --bin solvent

# 5. Maker liquidity, then a health check of the whole read API.
pnpm --dir scripts seed
pnpm --dir scripts smoke
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
| `etch` | `anvil_setCode`s `DevMulticall3`'s runtime bytecode to the canonical Multicall3 address. |
| `seed` | Mints, approves Aqua, and ships one strategy per pair. **Idempotent** — Aqua rejects re-shipping, so an existing position is skipped. |
| `smoke` | Calls the read API through the SDK client and prints a status matrix. Exits non-zero on any failure. |

## Notes

- **The seed is the real write path**: it builds programs with the same `@solvent/sdk` construction
  and positions modules a maker client uses, so a break here is a genuine break.
- The deploy manifest is bind-mounted to `contracts/deployments/` so the host can generate config
  from it; the chain itself persists in the `anvil-state` volume.
- `seed` runs under a resolver hook (`src/lib/register.mjs`): the published `@1inch` ESM imports its
  own files without extensions, which Node's resolver rejects.
- Generated config, the token list, the signing env, the database, and the manifest are untracked.
- **Permit2 is not placed yet.** Its `pragma 0.8.17` conflicts with the repo's `solc 0.8.30`, so it
  cannot be compiled here; the order-signing flow needs it.
