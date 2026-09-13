/** Starts/stops the long-running processes a deployed stack needs: two `solvent` backends and the
 *  local CCIP-mock relay. Each start is preceded by a port-ownership check (never blindly bind
 *  over something already listening) and followed by a health-check retry loop, so "the script
 *  finished" and "the service is actually answering" are never conflated. */
import { spawn, type ChildProcess } from "node:child_process";
import { mkdirSync, openSync } from "node:fs";
import { resolve } from "node:path";

import { REPO_ROOT } from "../lib/manifest.ts";
import { isPortOpen } from "./preflight.ts";
import { COORDINATOR_PORT } from "./config.ts";

export interface RunningProcess {
  label: string;
  child: ChildProcess;
  logPath: string;
}

function logFileFor(name: string): string {
  const dir = resolve(REPO_ROOT, "devnet/generated/crosschain/logs");
  mkdirSync(dir, { recursive: true });
  return resolve(dir, `${name}.log`);
}

async function waitHealthy(
  label: string,
  check: () => Promise<boolean>,
  timeoutMs: number,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await check()) return;
    await new Promise((r) => setTimeout(r, 1_000));
  }
  throw new Error(`${label} did not become healthy within ${timeoutMs}ms — check its log`);
}

/** Builds once (so the health-check loop below isn't racing a multi-minute compile) and starts
 *  the backend for one side, refusing to start if its port is already held by something that
 *  isn't a prior instance of this exact script's own tracked process. */
export interface BackendProcessConfig {
  label: string;
  apiPort: number;
  configName: string;
  envPath: string;
}

export async function startBackend(config: BackendProcessConfig): Promise<RunningProcess> {
  if (await isPortOpen(config.apiPort)) {
    throw new Error(
      `[${config.label}] :${config.apiPort} is already answering — a backend is already running here. ` +
        "Stop it first (this script does not assume ownership of a port it did not open).",
    );
  }

  console.log(`[${config.label}] building solvent (first run only takes a while)...`);
  await new Promise<void>((resolvePromise, reject) => {
    const build = spawn("cargo", ["build", "--bin", "solvent"], {
      cwd: REPO_ROOT,
      stdio: "inherit",
    });
    build.on("close", (code) =>
      code === 0 ? resolvePromise() : reject(new Error(`cargo build exited ${code}`)),
    );
  });

  const logPath = logFileFor(`solvent-${config.label}`);
  const log = openSync(logPath, "a");
  const child = spawn("./target/debug/solvent", [], {
    cwd: REPO_ROOT,
    env: {
      ...process.env,
      SOLVENT_CONFIG: config.configName,
      // `.env.sh` sets keys via `export`; load it into this child's own environment directly.
      ...(await loadEnvFile(config.envPath)),
    },
    stdio: ["ignore", log, log],
    detached: true,
  });
  child.unref();
  console.log(`[${config.label}] solvent starting (pid ${child.pid}), log: ${logPath}`);

  await waitHealthy(
    `[${config.label}] solvent :${config.apiPort}`,
    async () => {
      try {
        const res = await fetch(`http://127.0.0.1:${config.apiPort}/healthz`);
        return res.ok;
      } catch {
        return false;
      }
    },
    60_000,
  );
  console.log(`[${config.label}] solvent healthy on :${config.apiPort}`);
  return { label: `solvent-${config.label}`, child, logPath };
}

export async function startCoordinator(
  configPath: string,
  internalToken: string,
): Promise<RunningProcess> {
  if (await isPortOpen(COORDINATOR_PORT)) {
    throw new Error(
      `[coordinator] :${COORDINATOR_PORT} is already answering — stop it first with ` +
        "`pnpm --dir scripts run deploy:down`.",
    );
  }

  console.log("[coordinator] building solvent-proxy (first run only takes a while)...");
  await new Promise<void>((resolvePromise, reject) => {
    const build = spawn("cargo", ["build", "--bin", "solvent-proxy"], {
      cwd: REPO_ROOT,
      stdio: "inherit",
    });
    build.on("close", (code) =>
      code === 0 ? resolvePromise() : reject(new Error(`cargo build exited ${code}`)),
    );
  });

  const logPath = logFileFor("solvent-proxy");
  const log = openSync(logPath, "a");
  const child = spawn("./target/debug/solvent-proxy", [], {
    cwd: REPO_ROOT,
    env: {
      ...process.env,
      SOLVENT_PROXY_CONFIG: configPath.replace(/\.toml$/, ""),
      SOLVENT_INTERNAL_TOKEN: internalToken,
    },
    stdio: ["ignore", log, log],
    detached: true,
  });
  child.unref();
  console.log(`[coordinator] starting (pid ${child.pid}), log: ${logPath}`);

  await waitHealthy(
    `[coordinator] :${COORDINATOR_PORT}`,
    async () => {
      try {
        const res = await fetch(`http://127.0.0.1:${COORDINATOR_PORT}/healthz`);
        return res.ok;
      } catch {
        return false;
      }
    },
    60_000,
  );
  console.log(`[coordinator] healthy on :${COORDINATOR_PORT}`);
  return { label: "solvent-proxy", child, logPath };
}

/** Parses `export KEY=value` lines from a generated env file into a plain object, without
 *  shelling out to `sh -c 'source ...'` (keeps this cross-platform and injection-free). */
async function loadEnvFile(path: string): Promise<Record<string, string>> {
  const { readFileSync } = await import("node:fs");
  const out: Record<string, string> = {};
  for (const line of readFileSync(path, "utf8").split("\n")) {
    const match = /^export\s+([A-Z_][A-Z0-9_]*)=(.*)$/.exec(line.trim());
    if (match) out[match[1]] = match[2];
  }
  return out;
}

export function startRelay(): RunningProcess {
  const logPath = logFileFor("relay");
  const log = openSync(logPath, "a");
  // Absolute path, deliberately: `--down` (in a separate process invocation, with no PID to go on)
  // finds this by `pgrep -f` against the exact same absolute string — a relative arg here would
  // show up relative in `ps` too and silently never match.
  const child = spawn("node", [resolve(REPO_ROOT, "scripts/src/crosschain/relay.ts")], {
    cwd: resolve(REPO_ROOT, "scripts"),
    stdio: ["ignore", log, log],
    detached: true,
  });
  child.unref();
  console.log(`relay: starting (pid ${child.pid}), log: ${logPath}`);
  return { label: "relay", child, logPath };
}

export function stopAll(processes: RunningProcess[]): void {
  for (const p of processes) {
    if (p.child.pid) {
      try {
        process.kill(p.child.pid);
        console.log(`stopped ${p.label} (pid ${p.child.pid})`);
      } catch {
        // already gone
      }
    }
  }
}
