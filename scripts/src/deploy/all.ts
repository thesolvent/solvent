/**
 * One-click two-chain deploy: infra up, same-chain stacks, cross-chain wiring, liquidity, config,
 * backends, relay, smoke test. Safe to rerun after any failure — every phase verifies its own
 * manifest against live chain state before deciding to skip it.
 *
 *   node src/deploy/all.ts            # deploy (or resume an interrupted deploy)
 *   node src/deploy/all.ts --reset    # tear down and redeploy from a clean chain state
 *   node src/deploy/all.ts --down     # stop everything this script started; keep chain state
 */
import { execFile } from "node:child_process";
import { existsSync, readFileSync, rmSync } from "node:fs";
import { resolve } from "node:path";
import { promisify } from "node:util";

import { REPO_ROOT } from "../lib/manifest.ts";
import { etchCanonicalInfra } from "../devnet/etch.ts";
import { PreflightError, preflight } from "./preflight.ts";
import {
  deployCrossChainInfra,
  deployCrossWiredApps,
  configureProofRails,
  deployOriginCompact,
  deploySameChainStack,
  type ChainTarget,
} from "./phases.ts";
import {
  COORDINATOR_PORT,
  DIRECT_DESTINATION_PORTS,
  generateInternalToken,
  writeCoordinatorConfig,
  writeSideConfig,
  type DirectRouteConfig,
  PORTS,
} from "./config.ts";
import {
  startBackend,
  startCoordinator,
  startRelay,
  stopAll,
  type RunningProcess,
} from "./processes.ts";
import { CROSSCHAIN_ROOT, ensureManifestDirs } from "./manifests.ts";

const execFileAsync = promisify(execFile);
const COMPOSE = ["compose", "-f", "devnet/docker-compose.crosschain.yml"];

const ORIGIN: ChainTarget = {
  side: "origin",
  rpcUrl: PORTS.origin.rpcUrl,
  internalRpcUrl: "http://anvil-origin:8545",
  chainId: 31337,
};
interface DirectSeed {
  maker: string;
  strategyHash: string;
}

function directSeed(side: "origin" | "destination"): DirectSeed {
  const path = resolve(REPO_ROOT, "devnet/generated/crosschain", `direct-${side}.json`);
  try {
    const parsed = JSON.parse(readFileSync(path, "utf8")) as DirectSeed;
    if (!parsed.maker || !parsed.strategyHash) throw new Error("missing maker or strategy hash");
    return parsed;
  } catch (error) {
    throw new Error(`could not read ${side} direct route seed: ${error instanceof Error ? error.message : String(error)}`);
  }
}

const DESTINATION: ChainTarget = {
  side: "destination",
  rpcUrl: PORTS.destination.rpcUrl,
  internalRpcUrl: "http://anvil-destination:8546",
  chainId: 31338,
};

function compose(...args: string[]): Promise<{ stdout: string; stderr: string }> {
  return execFileAsync("docker", [...COMPOSE, ...args], { cwd: REPO_ROOT });
}

/** Docker Desktop can restore previously-running compose stacks (including this repo's other
 *  devnet stacks) right after it restarts — a race preflight's port check cannot fully close,
 *  since Docker itself can grab a port between that check and this call. One retry after a short
 *  pause absorbs that specific race; a second failure is a real conflict. */
async function bringUpInfra(): Promise<void> {
  console.log("== infra: docker compose up ==");
  try {
    await compose("up", "-d");
  } catch (error) {
    const message = String((error as { message?: string }).message ?? error);
    if (!/port is already allocated/i.test(message)) throw error;
    console.log(
      "  port bind race on first attempt (often another compose project settling after a " +
        "Docker restart) — waiting 5s and retrying once...",
    );
    await new Promise((r) => setTimeout(r, 5_000));
    try {
      await compose("up", "-d");
    } catch (retryError) {
      const retryMessage = String((retryError as { message?: string }).message ?? retryError);
      throw new Error(
        `${retryMessage}\n\nAnother docker-compose project is holding one of these ports. If ` +
          "that's this repo's single-chain devnet, stop it first: " +
          "`docker compose -f devnet/docker-compose.yml down`",
      );
    }
  }
}

async function tearDownInfra(): Promise<void> {
  console.log("== infra: docker compose down -v ==");
  await compose("down", "-v");
}

/** A price feed connects over Binance's public WebSocket after the backend reports healthy — its
 *  first tick can take a few seconds, and until it arrives a non-pegged pair (LINK, here) has no
 *  USD price to seed against. `/healthz` cannot see this (it is a liveness check, not "every
 *  subsystem is warm"), so retry the class of failure this produces instead of guessing a fixed
 *  delay long enough to always cover it. */
function isPriceFeedWarmup(output: string): boolean {
  return /no USD price for/i.test(output);
}

async function runSideScript(
  script: string,
  side: "origin" | "destination",
  extraEnv: Record<string, string> = {},
  retries = 1,
): Promise<void> {
  const ports = PORTS[side];
  for (let attempt = 1; attempt <= retries; attempt++) {
    try {
      const { stdout } = await execFileAsync("node", [script], {
        cwd: resolve(REPO_ROOT, "scripts"),
        env: {
          ...process.env,
          SOLVENT_RPC_URL: ports.rpcUrl,
          SOLVENT_API_URL: `http://127.0.0.1:${ports.apiPort}`,
          SOLVENT_MANIFEST: resolve(CROSSCHAIN_ROOT, side, "devnet.json"),
          SOLVENT_TRADE_REPORT: resolve(CROSSCHAIN_ROOT, side, "review-trades.json"),
          ...extraEnv,
        },
      });
      console.log(stdout.trimEnd());
      return;
    } catch (error) {
      const stdout = (error as { stdout?: string }).stdout ?? "";
      const stderr = (error as { stderr?: string }).stderr ?? "";
      const output = `${stdout}${stderr}`;
      if (attempt < retries && isPriceFeedWarmup(output)) {
        console.log(
          `  ${script} [${side}]: price feed still warming up, retrying (${attempt}/${retries})...`,
        );
        await new Promise((r) => setTimeout(r, 5_000));
        continue;
      }
      throw new Error(`${script} [${side}] failed:\n${output}`);
    }
  }
}

async function deployChain(target: ChainTarget) {
  console.log(`\n== [${target.side}] etch canonical infra ==`);
  await etchCanonicalInfra(target.rpcUrl);
  console.log(`\n== [${target.side}] phase 1: same-chain stack ==`);
  const devnet = await deploySameChainStack(target);
  console.log(`\n== [${target.side}] phase 2: crosschain infra ==`);
  const infra = await deployCrossChainInfra(target);
  return { devnet, infra };
}

async function main(): Promise<void> {
  const args = new Set(process.argv.slice(2));

  if (args.has("--down")) {
    console.log("Stopping backends, coordinator, and relay (chain state and containers are left running)...");
    // A separate invocation does not have the PIDs a prior `deploy` run printed — recover them by
    // identity instead: whatever is bound to *our* two API ports is, unambiguously, the backend
    // this script started (nothing else was allowed to hold them), and the relay is identified by
    // its exact script path, not a loose name match that could catch an unrelated node process.
    for (const port of [
      PORTS.origin.apiPort,
      PORTS.destination.apiPort,
      DIRECT_DESTINATION_PORTS.apiPort,
      COORDINATOR_PORT,
    ]) {
      try {
        const { stdout } = await execFileAsync("lsof", ["-ti", `:${port}`]);
        for (const pid of stdout.trim().split("\n").filter(Boolean)) {
          process.kill(Number(pid));
          console.log(`  stopped solvent on :${port} (pid ${pid})`);
        }
      } catch {
        console.log(`  nothing listening on :${port}`);
      }
    }
    try {
      const relayScript = resolve(REPO_ROOT, "scripts/src/crosschain/relay.ts");
      const { stdout } = await execFileAsync("pgrep", ["-f", relayScript]);
      for (const pid of stdout.trim().split("\n").filter(Boolean)) {
        process.kill(Number(pid));
        console.log(`  stopped relay (pid ${pid})`);
      }
    } catch {
      console.log("  relay was not running");
    }
    return;
  }

  if (args.has("--reset")) {
    console.log("Resetting: tearing down infra and clearing generated state...");
    await tearDownInfra().catch(() => undefined);
    if (existsSync(CROSSCHAIN_ROOT)) rmSync(CROSSCHAIN_ROOT, { recursive: true, force: true });
    for (const configName of ["solvent.origin", "solvent.destination", "solvent.destination-direct"]) {
      const cfg = resolve(REPO_ROOT, `${configName}.toml`);
      if (existsSync(cfg)) rmSync(cfg);
    }
  }

  await preflight({
    ports: [
      { port: 9645, label: "anvil-origin RPC" },
      { port: 9646, label: "anvil-destination RPC" },
      { port: 5300, label: "explorer-origin" },
      { port: 5301, label: "explorer-destination" },
      { port: 9181, label: "faucet-origin" },
      { port: 9182, label: "faucet-destination" },
      { port: PORTS.origin.apiPort, label: "solvent-origin API" },
      { port: PORTS.destination.apiPort, label: "solvent-destination API" },
      { port: COORDINATOR_PORT, label: "SolventX coordinator" },
      { port: DIRECT_DESTINATION_PORTS.apiPort, label: "direct destination API" },
    ],
  });

  ensureManifestDirs();
  await bringUpInfra();

  const { devnet: originDevnet, infra: originInfra } = await deployChain(ORIGIN);
  const { devnet: destinationDevnet, infra: destinationInfra } = await deployChain(DESTINATION);

  console.log("\n== phase 3: origin compact ==");
  const originCompact = await deployOriginCompact(ORIGIN);

  console.log("\n== phase 4: cross-wired apps (origin settler <-> destination app) ==");
  const { settler, app } = await deployCrossWiredApps(
    ORIGIN,
    DESTINATION,
    originDevnet,
    destinationDevnet,
    originInfra,
    destinationInfra,
    originCompact,
  );
  console.log("\n== phase 5: proof-rail links ==");
  await configureProofRails(
    ORIGIN,
    DESTINATION,
    originInfra,
    destinationInfra,
    settler,
    app,
  );

  console.log("\n== config: generating solvent.origin.toml / solvent.destination.toml ==");
  const internalToken = generateInternalToken();
  const wiring = { originSettler: settler.settler, destinationApp: app.app, internalToken };
  const originConfig = writeSideConfig("origin", originDevnet, originInfra, wiring);
  const destinationConfig = writeSideConfig(
    "destination",
    destinationDevnet,
    destinationInfra,
    wiring,
  );
  console.log("\n== starting chain-local backends ==");
  const running: RunningProcess[] = [];
  running.push(
    await startBackend({
      label: "origin",
      apiPort: PORTS.origin.apiPort,
      configName: "solvent.origin",
      envPath: originConfig.envPath,
    }),
  );
  running.push(
    await startBackend({
      label: "destination",
      apiPort: PORTS.destination.apiPort,
      configName: "solvent.destination",
      envPath: destinationConfig.envPath,
    }),
  );

  // The seed script talks to the live API (asset list, chain-id cross-check), not just the RPC —
  // it has to run after the backend is up and healthy, not before.
  console.log("\n== liquidity: seeding maker positions on both chains ==");
  await runSideScript("src/seed/strategies.ts", "origin", {}, 4);
  await runSideScript("src/seed/strategies.ts", "destination", {}, 4);

  console.log("\n== smoke test: same-chain swaps on both sides ==");
  await runSideScript("src/seed/trades.ts", "origin");
  await runSideScript("src/seed/trades.ts", "destination");
  await runSideScript("src/smoke/endpoints.ts", "origin");
  await runSideScript("src/smoke/endpoints.ts", "destination");

  console.log("\n== direct route: seed WBTC origin -> USDC destination ==");
  await runSideScript("src/seed/crosschain-direct.ts", "origin", {
    SOLVENT_DIRECT_SIDE: "origin",
    SOLVENT_STRATEGY_APP: settler.settler,
    SOLVENT_DIRECT_OUTPUT: "devnet/generated/crosschain/direct-origin.json",
  }, 4);
  await runSideScript("src/seed/crosschain-direct.ts", "destination", {
    SOLVENT_DIRECT_SIDE: "destination",
    SOLVENT_STRATEGY_APP: app.app,
    SOLVENT_DIRECT_OUTPUT: "devnet/generated/crosschain/direct-destination.json",
  }, 4);
  const originDirect = directSeed("origin");
  const destinationDirect = directSeed("destination");
  if (originDirect.maker.toLowerCase() !== destinationDirect.maker.toLowerCase()) {
    throw new Error("direct origin and destination positions must share one maker");
  }
  const directRoute: DirectRouteConfig = {
    maker: destinationDirect.maker,
    originStrategyHash: originDirect.strategyHash,
    originSettler: settler.settler,
    compact: originCompact.compact,
    compactLockTag: originCompact.lock_tag,
    destinationApp: app.app,
    originProofOutbox: originInfra.outbox,
    destinationProofOutbox: destinationInfra.outbox,
    destinationFillVerifier: destinationInfra.inbox,
    originChainId: originDevnet.chain_id,
    destinationChainId: destinationDevnet.chain_id,
  };
  const directDestinationConfig = writeSideConfig(
    "destination",
    destinationDevnet,
    destinationInfra,
    wiring,
    {
      configName: "solvent.destination-direct",
      appAddress: app.app,
      ports: DIRECT_DESTINATION_PORTS,
      databaseName: "direct-solvent.db",
      directRoute,
    },
  );
  const coordinatorConfig = writeCoordinatorConfig(directRoute);

  console.log("\n== starting isolated direct destination + coordinator + relay ==");
  running.push(
    await startBackend({
      label: "destination-direct",
      apiPort: DIRECT_DESTINATION_PORTS.apiPort,
      configName: "solvent.destination-direct",
      envPath: directDestinationConfig.envPath,
    }),
  );
  running.push(await startCoordinator(coordinatorConfig.configPath, internalToken));
  running.push(startRelay());

  console.log("\n== smoke test: direct WBTC origin -> USDC destination ==");
  await runSideScript("src/smoke/crosschain-direct.ts", "origin", {
    SOLVENT_COORDINATOR_URL: `http://127.0.0.1:${COORDINATOR_PORT}`,
  });

  console.log("\n== cross-chain infra summary ==");
  console.log(`  origin settler:      ${settler.settler} (chain ${originDevnet.chain_id})`);
  console.log(`  destination app:     ${app.app} (chain ${destinationDevnet.chain_id})`);
  console.log(
    "  direct lane: WBTC on origin -> USDC on destination via the isolated direct destination service.",
  );

  console.log("\n== done ==");
  console.log(`  origin:      API http://127.0.0.1:${PORTS.origin.apiPort} · explorer http://127.0.0.1:5300 · faucet http://127.0.0.1:9181`);
  console.log(`  destination: API http://127.0.0.1:${PORTS.destination.apiPort} · explorer http://127.0.0.1:5301 · faucet http://127.0.0.1:9182`);
  console.log(`  direct destination: API http://127.0.0.1:${DIRECT_DESTINATION_PORTS.apiPort}`);
  console.log(`  coordinator: http://127.0.0.1:${COORDINATOR_PORT}`);
  console.log(`  relay + backend logs: devnet/generated/crosschain/logs/`);
  console.log(`  pids: ${running.map((p) => `${p.label}=${p.child.pid}`).join(" ")}`);
}

main().catch((error: unknown) => {
  if (error instanceof PreflightError) {
    console.error(`\npreflight failed: ${error.message}`);
  } else {
    console.error(`\ndeploy failed: ${error instanceof Error ? error.stack ?? error.message : error}`);
  }
  process.exitCode = 1;
});
