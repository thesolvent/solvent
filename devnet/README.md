# Solvent devnet

A self-contained local EVM devnet for reviewing Solvent end to end: a chain, a block explorer, our
deployed contracts + a set of test tokens, and a faucet. One command, no external services.

```bash
just devnet-up        # boot everything (builds the faucet image on first run)
just devnet-logs seed # watch the deploy finish
just devnet-smoke     # assert finality advances, faucet drips, explorer is up
just devnet-down      # stop and wipe (drops the deploy manifest)
```

## What comes up

| Service   | Port  | What it is |
|-----------|-------|------------|
| `anvil-1` | 8545  | Anvil EVM, chain id `31337`, 1s blocks. Runs with `--disable-code-size-limit` (the full `AquaSwapVMRouter` exceeds EIP-170) and `--slots-in-an-epoch 1` (so the `finalized` tag advances and the resolver's finality-anchored confirmation settles). |
| `explorer`| 5100  | [Otterscan](https://github.com/otterscan/otterscan) against anvil's `ots_*` RPC — no indexer. |
| `seed`    | —     | One-shot: installs the canonical Permit2 and Multicall3 runtimes with Anvil’s local-only code override, deploys Aqua + the SwapVM router + the protocol fillers and the Core-6 test tokens, then writes the address manifest. Exits when done. |
| `faucet`  | 8081  | `POST /faucet {"address":"0x…","tokens":["USDC",…]?}` — mints test tokens (all by default) and tops up gas, rate-limited per address. |

### Test tokens (Core-6, real decimals)

`USDC` (6), `USDT` (6), `DAI` (18), `WETH` (18), `WBTC` (8), `LINK` (18). Deployed fresh each boot;
their addresses are in the manifest.

### The address manifest

The seed writes `solvent-devnet.json` (chain id, Permit2/Multicall3, Aqua/router/reactor/filler, and
the token table) into the shared `manifest` volume. The faucet reads it; the backend and frontend
will too.

## Notes

- The deployer, filler, policy signer, cosigner, and faucet each use separate pre-funded Anvil
  accounts. The faucet uses its own durable WalletKit state store, so concurrent drips do not
  share a nonce manager with the backend. These are well-known throwaway keys, **devnet only**.
- Permit2 and Multicall3 keep their canonical addresses. The seed service installs their pinned
  runtimes with Anvil’s local-only `anvil_setCode` RPC before it deploys contracts that validate
  those dependencies.
- wharfnet is **not** a runtime dependency. The anvil flags above match what our upstream wharfnet
  `feat/anvil-extra-args` contribution enables; that PR stands on its own and is unrelated to running
  this devnet.

## Deploying to Coolify

The stack is a plain compose, so Coolify deploys it as a Docker Compose resource pointed at this
repo. Two devnet-specific adjustments:

1. **Seed source.** Locally the `seed` service bind-mounts `../contracts` (which already has its
   Foundry deps installed). A Coolify host has no such bind mount, so bake a seed image instead — a
   `Dockerfile` on `foundry` that `COPY`s `contracts/` (with `node_modules` + `lib` from `just
   setup`) and runs `seed.sh` — and point the `seed` service at it. The `manifest` named volume and
   the faucet stay as-is.
2. **Persistence.** The `manifest` volume is a named Docker volume, so it survives restarts. Anvil
   itself is ephemeral (fresh chain each boot); add `--state /state/anvil.json` + a volume if you
   want the chain to persist too.

Expose ports 8545 (RPC), 5100 (explorer), and 8081 (faucet). The faucet image builds from
`crates/devnet/Dockerfile` and needs network access at build time to fetch dependencies.
