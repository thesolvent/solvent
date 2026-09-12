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
import { existsSync, rmSync } from "node:fs";
import { resolve } from "node:path";
import { promisify } from "node:util";

import { REPO_ROOT } from "../lib/manifest.ts";
import { etchCanonicalInfra } from "../devnet/etch.ts";
import { PreflightError, preflight } from "./preflight.ts";
import {
  deployCrossChainInfra,
  deployCrossWiredApps,
  deployOriginCompact,
  deploySameChainStack,
  type ChainTarget,
} from "./phases.ts";
import { generateInternalToken, writeSideConfig, PORTS } from "./config.ts";
import { startBackend, startRelay, stopAll, type RunningProcess } from "./processes.ts";
import { CROSSCHAIN_ROOT, ensureManifestDirs } from "./manifests.ts";

const execFileAsync = promisify(execFile);
const COMPOSE = ["compose", "-f", "devnet/docker-compose.crosschain.yml"];

const ORIGIN: ChainTarget = {
  side: "origin",
  rpcUrl: PORTS.origin.rpcUrl,
  internalRpcUrl: "http://anvil-origin:8545",
  chainId: 31337,
};
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
    console.log("Stopping backends + relay (chain state and containers are left running)...");
    // A separate invocation does not have the PIDs a prior `deploy` run printed — recover them by
    // identity instead: whatever is bound to *our* two API ports is, unambiguously, the backend
    // this script started (nothing else was allowed to hold them), and the relay is identified by
    // its exact script path, not a loose name match that could catch an unrelated node process.
    for (const port of [PORTS.origin.apiPort, PORTS.destination.apiPort]) {
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
    for (const side of ["origin", "destination"] as const) {
      const cfg = resolve(REPO_ROOT, `solvent.${side}.toml`);
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

  console.log("\n== starting backends + relay ==");
  const running: RunningProcess[] = [];
  running.push(await startBackend("origin", originConfig.envPath));
  running.push(await startBackend("destination", destinationConfig.envPath));
  running.push(startRelay());

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

  console.log("\n== cross-chain infra summary ==");
  console.log(`  origin settler:      ${settler.settler} (chain ${originDevnet.chain_id})`);
  console.log(`  destination app:     ${app.app} (chain ${destinationDevnet.chain_id})`);
  console.log(
    "  NOTE: this verifies the infra is deployed, wired, and both chains' same-chain swap " +
      "paths work. A full signed cross-chain swap is not scriptable yet — the client-side " +
      "signed Compact claim construction the routed lane needs is not built (see " +
      "scripts/test-crosschain-direct-e2e.sh's own note on this boundary). Use " +
      "`cargo test -p solvent-adapters --test e2e_crosschain_direct` for that proof today.",
  );

  console.log("\n== done ==");
  console.log(`  origin:      API http://127.0.0.1:${PORTS.origin.apiPort} · explorer http://127.0.0.1:5300 · faucet http://127.0.0.1:9181`);
  console.log(`  destination: API http://127.0.0.1:${PORTS.destination.apiPort} · explorer http://127.0.0.1:5301 · faucet http://127.0.0.1:9182`);
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
