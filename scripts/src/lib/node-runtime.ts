import { createRequire } from "node:module";

const require = createRequire(import.meta.url);

// Upstream SDK ESM assumes a bundler. Its Node clients must share viem's CommonJS error constructors.
export const { Strategy, linearWidthFromSymmetricRangePercent } =
    require("@solvent/sdk/construction") as typeof import("@solvent/sdk/construction");
export const { positions } =
    require("@solvent/sdk/positions") as typeof import("@solvent/sdk/positions");
export const { createSwapClient } =
    require("@solvent/sdk/swap") as typeof import("@solvent/sdk/swap");
export const {
    createPublicClient,
    createTestClient,
    createWalletClient,
    http,
} = require("viem") as typeof import("viem");
