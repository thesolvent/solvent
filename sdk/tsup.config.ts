import { defineConfig } from "tsup";

export default defineConfig({
    entry: {
        index: "src/index.ts",
        construction: "src/construction/index.ts",
        positions: "src/positions/index.ts",
        client: "src/client/index.ts",
        orders: "src/orders/index.ts",
        swap: "src/swap/index.ts",
        "cross-chain": "src/cross-chain/index.ts",
        validation: "src/validation.ts",
    },
    format: ["esm", "cjs"],
    dts: true,
    clean: true,
    sourcemap: true,
    treeshake: true,
});
