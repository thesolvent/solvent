/** Runs one `forge script` contract against a live RPC, with the retry/error-classification a
 *  one-click deploy needs: a transient RPC hiccup (the chain container was still warming up, a
 *  dropped connection) is worth retrying; a revert is not — retrying a revert just burns a nonce
 *  and gas for the same failure, so it fails immediately and lets the caller decide what to do.
 *
 *  Runs `forge` inside the same `ghcr.io/foundry-rs/foundry:stable` image the single-chain
 *  devnet's `seed.sh` already uses, not the host's `forge` install: the host had a newer version
 *  (1.7.1) whose script-simulation step enforces the EIP-3860 init-code-size limit strictly, which
 *  reverted deploying `UniswapXAquaFiller` even though the exact same contract deploys fine
 *  against a real (or `--disable-code-size-limit`) chain — the pinned image's 1.5.1 does not hit
 *  this, and it is the version already proven to work for this repo's contracts. */
import { spawn } from "node:child_process";

import { REPO_ROOT } from "../lib/manifest.ts";

export class ForgeRevertError extends Error {}

export interface RunForgeScriptOptions {
  /** Path to the .sol file, relative to `contracts/`. */
  scriptPath: string;
  contract: string;
  /** The RPC URL reachable *from inside the docker network* (a container DNS name, not
   *  `127.0.0.1`) — this always runs in a container attached to `solvent-crosschain-swapux_default`. */
  rpcUrl: string;
  privateKey: string;
  /** Extra env vars the script's `vm.env*` calls read. Paths in these must be container paths
   *  (under `/repo`), not host paths — see `toContainerPath`. */
  env?: Record<string, string>;
  retries?: number;
}

const IMAGE = "ghcr.io/foundry-rs/foundry:stable";
// Dedicated to this branch's crosschain stack — never the same network as the pr14-merge-master
// worktree's own crosschain deploy (`solvent-crosschain_default`), so the two can run side by side.
const NETWORK = "solvent-crosschain-swapux_default";

/** `/repo`-relative path for anything under REPO_ROOT — the manifest/env paths this module's
 *  callers pass are host-absolute, but the script runs with REPO_ROOT bind-mounted at `/repo`. */
export function toContainerPath(hostPath: string): string {
  if (!hostPath.startsWith(REPO_ROOT)) {
    throw new Error(`${hostPath} is not under REPO_ROOT (${REPO_ROOT}) — cannot mount it`);
  }
  return `/repo${hostPath.slice(REPO_ROOT.length)}`;
}

function isTransient(output: string): boolean {
  return (
    /error sending request/i.test(output) ||
    /connection refused/i.test(output) ||
    /timed?\s*out/i.test(output) ||
    /EOF/i.test(output)
  );
}

export async function runForgeScript(opts: RunForgeScriptOptions): Promise<string> {
  const retries = opts.retries ?? 3;
  let lastOutput = "";
  const envArgs = Object.entries(opts.env ?? {}).flatMap(([k, v]) => ["-e", `${k}=${v}`]);
  for (let attempt = 1; attempt <= retries; attempt++) {
    const result = await new Promise<{ code: number | null; output: string }>(
      (resolvePromise) => {
        const child = spawn(
          "docker",
          [
            "run",
            "--rm",
            "--network",
            NETWORK,
            "-v",
            `${REPO_ROOT}:/repo`,
            "-w",
            "/repo/contracts",
            ...envArgs,
            "--entrypoint",
            "forge",
            IMAGE,
            "script",
            `${opts.scriptPath}:${opts.contract}`,
            "--broadcast",
            "--rpc-url",
            opts.rpcUrl,
            "--private-key",
            opts.privateKey,
          ],
        );
        let output = "";
        child.stdout.on("data", (d: Buffer) => (output += d.toString()));
        child.stderr.on("data", (d: Buffer) => (output += d.toString()));
        child.on("close", (code) => resolvePromise({ code, output }));
      },
    );
    lastOutput = result.output;
    if (result.code === 0) return result.output;
    if (!isTransient(result.output) || attempt === retries) {
      throw new ForgeRevertError(
        `forge script ${opts.contract} failed (exit ${result.code}) on attempt ${attempt}/${retries}:\n${result.output.slice(-4_000)}`,
      );
    }
    console.log(
      `  ${opts.contract}: transient failure, retrying (${attempt}/${retries})...`,
    );
    await new Promise((r) => setTimeout(r, 2_000 * attempt));
  }
  throw new ForgeRevertError(`forge script ${opts.contract} failed:\n${lastOutput.slice(-4_000)}`);
}
