# One-click cross-chain deploy

`node src/deploy/all.ts` (or `just deploy-crosschain` from the repo root) takes two chains from
nothing to a fully seeded, fully running, smoke-tested pair of Solvent deployments in one command.

```sh
just deploy-crosschain          # deploy (or resume an interrupted deploy)
just deploy-crosschain-reset    # tear down and redeploy from a clean chain state
just deploy-crosschain-down     # stop the backends + relay this started; chain state stays up
```

Ports are deliberately disjoint from the single-chain devnet's (`docker-compose.yml`) AND from the
`pr14-merge-master` worktree's own crosschain deploy, so all three can run side by side:

| Side        | Chain RPC | Explorer | Faucet | API  | Internal crosschain listener |
|-------------|-----------|----------|--------|------|-------------------------------|
| origin      | 9645      | 5300     | 9181   | 8399 | 9380 |
| destination | 9646      | 5301     | 9182   | 8400 | 9381 |

## What it does

1. **Infra up** — `docker compose -f devnet/docker-compose.crosschain.yml up -d`: two anvil chains
   (31337 origin, 31338 destination), two Otterscan explorers, two faucets.
2. **Same-chain stack, both chains** — `DeployDevnet.s.sol` on each: Aqua, the SwapVM router, the
   UniswapX reactor + filler, the ERC-7683 settler/filler/resolver, the Core-6 test tokens. The
   filler's owner and policy signer are distinct anvil dev keys from the broadcasting deployer
   (`FILLER_OWNER_KEY`/`POLICY_SIGNER_KEY`) — matches the same-chain devnet's own seed service, and
   the generated `solvent.<side>.toml`'s signing env must use the same two keys or nothing the
   backend signs verifies against the deployed fillers.
3. **Cross-chain proof rail, both chains** — `DeployCrossChainInfra.s.sol`: a CCIP-mock router +
   proof inbox + proof outbox on each chain, each addressed to the other's fixed selector (`11` for
   origin, `22` for destination — must match `src/crosschain/relay.ts`).
4. **Origin Compact** — `TheCompact` + an always-allow allocator, origin only.
5. **The cross-wired pair** — `CompactOriginSettler` (origin) and `CrossChainAquaApp`
   (destination) each take the *other's* address as an immutable constructor argument. Both
   addresses are precomputed from each deployer's current nonce before either deploys, then the
   actual deployed address is checked against the prediction — the only way to break a two-sided
   immutable reference cycle these contracts don't have a redeploy-and-relink path for.
6. **Config** — one `solvent.<side>.toml` per chain, each with a `[crosschain]` block pointing at
   the other, plus a generated shared internal-auth token.
7. **Backends + relay** — one `solvent` process per chain (matches how `[crosschain]` config is
   already shaped: each deployment is chain-local, reaching its counterpart through the internal
   proxy), plus `src/crosschain/relay.ts` watching both chains' CCIP-mock routers and delivering
   messages between them (there is no real cross-chain messaging locally — this stands in for it).
8. **Liquidity + smoke test** — maker positions seeded on both chains' 8 core pairs, 24 real signed
   same-chain swaps executed and confirmed on *each* chain, then the full read-API smoke check on
   both.

**Scope note:** this proves both chains' infra is deployed and wired correctly, and that each
chain's own same-chain swap path works for real. It does not run a live, signed, end-to-end
cross-chain swap — the client-side construction of a signed Compact claim that the routed lane
needs is not built yet (see `scripts/test-crosschain-direct-e2e.sh`'s own note on this same
boundary). For that proof today, use `cargo test -p solvent-adapters --test e2e_crosschain_direct`.

## Why it's safe to rerun

Every phase writes its manifest only after every transaction in it lands — a killed process never
leaves a half-written one behind, so its mere absence already means "not done". On top of that,
each phase **re-verifies its manifest against live chain state** before deciding to skip: it checks
that the recorded addresses still hold real code on the chain right now, not just that a JSON file
exists. A stale manifest (wrong chain, wiped state, a port that silently pointed somewhere else)
is detected and that phase redeploys, rather than the whole run reporting success against state
that no longer exists.

This is not theoretical — building this (and the earlier crosschain deploy on `pr14-merge-master`,
which this port carries forward) surfaced exactly these failures:

- **A stray anvil process on the host**, left over from earlier manual testing, silently answered
  `127.0.0.1:8545` instead of the freshly deployed Docker container. Every check against that port
  was checking the wrong chain. Preflight now checks the *actual owning process* of every port this
  deploy needs, not just whether the port is free.
- **`lsof`'s COMMAND column truncates** `com.docker.backend` to `com.docke` — one character short
  of matching a `"docker"` substring check, so Docker's own port-forwarding proxy was misidentified
  as a conflicting process. Fixed by resolving the full command via `ps -p <pid> -o comm=` instead
  of trusting `lsof`'s truncated column.
- **Docker Desktop restores previously-running compose stacks after it restarts** — this deploy's
  ports are deliberately disjoint from every other devnet stack in this repo (including another
  worktree's crosschain deploy) so a restore can never collide with someone else's active work, and
  `bringUpInfra` treats a port-bind race as a named, retryable failure rather than silently
  colliding.
- **A newer host `forge` (1.7.1) enforces the EIP-3860 init-code-size limit** during `forge
  script`'s local simulation more strictly than the `ghcr.io/foundry-rs/foundry:stable` image's
  1.5.1 does, reverting `UniswapXAquaFiller`'s deployment even though the exact same contract
  deploys and runs fine against a real (or `--disable-code-size-limit`) chain. Every `forge script`
  call now runs inside that same pinned image, not the host binary.
- **The image's entrypoint is `/bin/sh -c`** — passing `forge script ...` as separate `docker run`
  arguments made them positional shell parameters (`$0`, `$1`, ...) rather than `forge`'s own argv,
  so `forge` silently ran with no arguments and printed its top-level help. Fixed with an explicit
  `--entrypoint forge` override.
- **`vm.writeJson` only has permission to write under `contracts/deployments/`** (`fs_permissions`
  in `foundry.toml`), and does not create parent directories. Manifest paths live under
  `contracts/deployments/crosschain/`, and the orchestrator creates both sides' directories before
  any `forge script` call.
- **`DeployDevnet` on this branch requires a filler owner and policy signer distinct from the
  deployer** (`FILLER_OWNER_KEY`/`POLICY_SIGNER_KEY`) — a signing-key split that landed after the
  original crosschain deploy port was built. The generated `solvent.<side>.toml`'s `SOLVENT_SIGNER_KEY`/
  `SOLVENT_POLICY_SIGNER_KEY` now match those same two keys, or the backend signs with an address
  the deployed fillers don't trust and every same-chain swap fails.
- **The seed script needs the live backend API**, not just chain RPC — it was originally sequenced
  before the backends started. Config generation and backend startup now happen first.
- **A non-pegged pair's price arrives over Binance's WebSocket** some seconds after the backend
  reports `/healthz` — a liveness check, not "every subsystem is warm." Seeding retries the specific
  "no USD price" failure a few times with a short delay instead of guessing a fixed startup delay.

## Layout

- `preflight.ts` — Docker readiness, port-ownership checks.
- `manifests.ts` — the per-phase address-book types and where they live on disk.
- `chain.ts` — host-side RPC helpers (nonce → predicted CREATE address, code-exists checks).
- `forge.ts` — runs one `forge script` contract inside the pinned Docker image.
- `phases.ts` — the four deploy phases, each idempotent.
- `config.ts` — generates the two `solvent.<side>.toml`s + signing env.
- `processes.ts` — starts/health-checks the two backends and the relay.
- `all.ts` — the orchestrator tying all of the above together.
