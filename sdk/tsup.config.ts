import { defineConfig } from "tsup";

export default defineConfig({
  entry: {
    index: "src/index.ts",
    construction: "src/construction/index.ts",
  },
  format: ["esm", "cjs"],
  dts: true,
  clean: true,
  sourcemap: true,
  treeshake: true,
});
