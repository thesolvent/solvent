/**
 * Preflight checks the one-click deploy runs before touching anything.
 *
 * Every check here exists because it bit us during manual testing: a stray host `anvil` left
 * over from an earlier session squatted on the same port Docker wanted, so every RPC call from
 * this script silently talked to the wrong chain for an hour before anyone noticed. These checks
 * turn that class of failure into a loud, actionable error before any deploy transaction is sent.
 */
import { execFile } from "node:child_process";
import net from "node:net";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);

export class PreflightError extends Error {}

async function commandExists(bin: string): Promise<boolean> {
  try {
    await execFileAsync("which", [bin]);
    return true;
  } catch {
    return false;
  }
}

/** Docker's own published ports are owned by its VM proxy, never a bare "docker" process name on
 *  macOS/Windows — allow anything under the `com.docker`/`docker` family through. */
function ownedByDocker(command: string): boolean {
  return /docker/i.test(command);
}

/** `ps`'s own `comm`/`command` output is untruncated, unlike `lsof`'s COMMAND column — which cuts
 *  "com.docker.backend" down to "com.docke", silently dropping the one character (the final "r")
 *  that a substring match for "docker" needs. Trusting lsof's column here produced false "port
 *  conflict" failures against Docker's own proxy on every run. */
async function fullCommandFor(pid: string): Promise<string> {
  try {
    const { stdout } = await execFileAsync("ps", ["-p", pid, "-o", "comm="]);
    return stdout.trim();
  } catch {
    return "";
  }
}

async function portOwner(
  port: number,
): Promise<{ pid: string; command: string } | null> {
  try {
    const { stdout } = await execFileAsync("lsof", [
      "-nP",
      `-iTCP:${port}`,
      "-sTCP:LISTEN",
    ]);
    const line = stdout.trim().split("\n").at(1); // skip the header row
    if (!line) return null;
    const fields = line.split(/\s+/);
    const pid = fields[1] ?? "?";
    const command = (await fullCommandFor(pid)) || fields[0] || "?";
    return { command, pid };
  } catch {
    return null; // lsof exits non-zero when nothing is listening
  }
}

/** True if `port` is free, or held only by our own previous docker-compose run. */
async function portAvailable(
  port: number,
): Promise<{ ok: true } | { ok: false; reason: string }> {
  const owner = await portOwner(port);
  if (!owner) return { ok: true };
  if (ownedByDocker(owner.command)) return { ok: true };
  return {
    ok: false,
    reason: `port ${port} is already held by ${owner.command} (pid ${owner.pid}), not Docker — kill it first: kill ${owner.pid}`,
  };
}

async function dockerReady(retries: number, delayMs: number): Promise<void> {
  for (let attempt = 1; attempt <= retries; attempt++) {
    try {
      await execFileAsync("docker", ["info", "--format", "{{.ServerVersion}}"]);
      return;
    } catch (cause) {
      if (attempt === retries) {
        throw new PreflightError(
          "Docker daemon is not reachable after " +
            `${retries} attempts (${(retries * delayMs) / 1000}s). ` +
            "Is Docker Desktop actually running (not just launched)? " +
            "`docker info` should return instantly once it is.",
          { cause },
        );
      }
      await new Promise((r) => setTimeout(r, delayMs));
    }
  }
}

export interface PreflightPlan {
  ports: { port: number; label: string }[];
}

export async function preflight(plan: PreflightPlan): Promise<void> {
  const missing: string[] = [];
  for (const bin of ["docker", "forge", "cast", "node"]) {
    if (!(await commandExists(bin))) missing.push(bin);
  }
  if (missing.length > 0) {
    throw new PreflightError(
      `missing required tool(s) on PATH: ${missing.join(", ")}`,
    );
  }

  console.log("preflight: waiting for the Docker daemon...");
  await dockerReady(24, 5_000); // up to 2 minutes — a cold Docker Desktop VM boot is genuinely slow
  console.log("preflight: Docker is up");

  const conflicts: string[] = [];
  for (const { port, label } of plan.ports) {
    const result = await portAvailable(port);
    if (!result.ok) conflicts.push(`  ${label} (:${port}) — ${result.reason}`);
  }
  if (conflicts.length > 0) {
    throw new PreflightError(
      `port conflict(s) that would silently misroute the deploy:\n${conflicts.join("\n")}`,
    );
  }
  console.log(`preflight: all ${plan.ports.length} required ports are free`);
}

/** A quick, low-cost "is anything listening" probe — used for the health-check retry loops, not
 *  preflight itself (which needs the sharper lsof-based ownership check above). */
export function isPortOpen(port: number, host = "127.0.0.1"): Promise<boolean> {
  return new Promise((resolve) => {
    const socket = net.createConnection({ port, host });
    socket.once("connect", () => {
      socket.destroy();
      resolve(true);
    });
    socket.once("error", () => resolve(false));
    socket.setTimeout(1_000, () => {
      socket.destroy();
      resolve(false);
    });
  });
}
