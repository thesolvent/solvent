import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    server: {
      deps: {
        // The published 1inch SDK ESM uses extensionless relative imports, which Node's strict
        // ESM loader rejects. Inlining lets vite's resolver handle them during tests.
        inline: [/@1inch\//],
      },
    },
  },
});
