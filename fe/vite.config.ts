import { fileURLToPath, URL } from "node:url";

import react from "@vitejs/plugin-react";
import { configDefaults, defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
      assert: fileURLToPath(new URL("./src/shims/assert.ts", import.meta.url)),
    },
  },
  server: {
    // Honour the harness-assigned port; 5173 may already be taken.
    port: process.env.PORT ? Number(process.env.PORT) : undefined,
    proxy: {
      "/solventx-api": {
        target: process.env.SOLVENTX_PROXY_TARGET ?? "http://127.0.0.1:8090",
        changeOrigin: true,
        rewrite: (path) => path.replace(/^\/solventx-api/, ""),
      },
    },
  },
  css: {
    modules: { localsConvention: "camelCaseOnly" },
  },
  test: {
    exclude: [...configDefaults.exclude, "e2e/**"],
    globals: true,
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    css: true,
    server: {
      deps: {
        inline: ["@solvent/sdk", "@1inch/swap-vm-sdk", "@1inch/byte-utils"],
      },
    },
  },
});
