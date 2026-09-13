/** Address-book types for the cross-chain deploy phases, and the paths they live at. Mirrors
 *  `../lib/manifest.ts`'s single-chain `Manifest`, split one file per phase so a phase's output is
 *  never ambiguous about whether it finished. */
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";

import { REPO_ROOT, type Manifest } from "../lib/manifest.ts";

// Must stay under `contracts/deployments/`: that is the only path `forge script`'s `vm.writeJson`
// is allowed to touch (`fs_permissions` in contracts/foundry.toml, scoped to `./deployments`).
export const CROSSCHAIN_ROOT = resolve(REPO_ROOT, "contracts/deployments/crosschain");

export type Side = "origin" | "destination";

export interface CrossChainInfraManifest {
  chain_id: number;
  router: string;
  inbox: string;
  outbox: string;
}

export interface OriginCompactManifest {
  compact: string;
  allocator: string;
  allocator_id: number;
  lock_tag: string;
}

export interface OriginSettlerManifest {
  settler: string;
}

export interface DestinationAppManifest {
  app: string;
}

function pathFor(side: Side, name: string): string {
  return resolve(CROSSCHAIN_ROOT, side, `${name}.json`);
}

export function devnetManifestPath(side: Side): string {
  return pathFor(side, "devnet");
}
export function infraManifestPath(side: Side): string {
  return pathFor(side, "crosschain-infra");
}
export function compactManifestPath(): string {
  return pathFor("origin", "compact");
}
export function settlerManifestPath(): string {
  return pathFor("origin", "settler");
}
export function appManifestPath(): string {
  return pathFor("destination", "app");
}

/** Read a phase's manifest, or `undefined` if that phase has not completed yet. A phase's script
 *  writes its manifest only after every transaction in it lands, so partial/interrupted runs never
 *  leave one behind — its absence is exactly "not done yet", nothing subtler. */
export function tryRead<T>(path: string): T | undefined {
  if (!existsSync(path)) return undefined;
  try {
    return JSON.parse(readFileSync(path, "utf8")) as T;
  } catch {
    return undefined; // truncated/corrupt write from a killed process — treat as absent, redo it
  }
}

export function write<T>(path: string, value: T): void {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`);
}

/** `vm.writeJson` does not create parent directories — they must exist before `forge script` runs,
 *  not just before *this* process's own `write()` calls above. */
export function ensureManifestDirs(): void {
  mkdirSync(resolve(CROSSCHAIN_ROOT, "origin"), { recursive: true });
  mkdirSync(resolve(CROSSCHAIN_ROOT, "destination"), { recursive: true });
}

export type { Manifest };
