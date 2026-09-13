import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

/** Repo root, derived from this file's location so scripts run from any cwd. */
export const REPO_ROOT = resolve(
    dirname(fileURLToPath(import.meta.url)),
    "../../..",
);

export const MANIFEST_PATH = resolve(
    REPO_ROOT,
    "contracts/deployments/solvent-devnet.json",
);

export interface ManifestToken {
    address: string;
    decimals: number;
}

/** The address book written by the devnet deploy script. */
export interface Manifest {
    aqua: string;
    chain_id: number;
    erc7683_filler: string;
    erc7683_resolver: string;
    erc7683_settler: string;
    filler: string;
    multicall3: string;
    permit2: string;
    reactor: string;
    router: string;
    taker_credential: string;
    tokens: Record<string, ManifestToken>;
}

export function readManifest(
    path: string = process.env.SOLVENT_MANIFEST ?? MANIFEST_PATH,
): Manifest {
    try {
        return JSON.parse(readFileSync(path, "utf8")) as Manifest;
    } catch (cause) {
        throw new Error(
            `no devnet manifest at ${path} — deploy the devnet first (forge script DeployDevnet)`,
            { cause },
        );
    }
}
