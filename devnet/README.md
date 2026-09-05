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
| `seed`    | —     | One-shot: deploys Aqua + the SwapVM router + the UniswapX reactor + our filler, and the Core-6 test tokens; writes the address manifest. Exits when done. (Permit2/Multicall3 sit at fixed canonical addresses and must be etched, not `new`-deployed; they land with execution in the next phase.) |
| `faucet`  | 8080  | `POST /faucet {"address":"0x…","tokens":["USDC",…]?}` — mints test tokens (all by default) and tops up gas, rate-limited per address. |

### Test tokens (Core-6, real decimals)

`USDC` (6), `USDT` (6), `DAI` (18), `WETH` (18), `WBTC` (8), `LINK` (18). Deployed fresh each boot;
their addresses are in the manifest.

### The address manifest

The seed writes `solvent-devnet.json` (chain id, Permit2/Multicall3, Aqua/router/reactor/filler, and
the token table) into the shared `manifest` volume. The faucet reads it; the backend and frontend
will too.

## Notes

- The deployer/minter is the standard anvil dev account #0 — a well-known throwaway key, **devnet
  only**. `mint` on the test tokens is unrestricted.
- Permit2 and Multicall3 are not placed here — they live at fixed canonical addresses that require
  etching (a node cheat, not a deploy), and nothing in S1 needs them; they arrive with execution.
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

Expose ports 8545 (RPC), 5100 (explorer), and 8080 (faucet). The faucet image builds from
`crates/devnet/Dockerfile` and needs network access at build time to fetch dependencies.
