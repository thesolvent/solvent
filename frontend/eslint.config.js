import js from "@eslint/js";
import boundaries from "eslint-plugin-boundaries";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";
import globals from "globals";
import tseslint from "typescript-eslint";

// Hexagonal boundaries: dependencies point inward only. A view reaches infrastructure/ports
// only through an application hook; application never imports concrete infrastructure; app/ is
// the one composition root that may wire everything. A violation fails the gate.
export default tseslint.config(
  { ignores: ["dist", "coverage", "node_modules"] },
  {
    files: ["**/*.{ts,tsx}"],
    extends: [js.configs.recommended, ...tseslint.configs.recommended],
    languageOptions: {
      ecmaVersion: 2022,
      globals: globals.browser,
    },
    plugins: {
      "react-hooks": reactHooks,
      "react-refresh": reactRefresh,
      boundaries,
    },
    settings: {
      "import/resolver": {
        typescript: { project: "./tsconfig.json" },
      },
      "boundaries/include": ["src/**/*"],
      "boundaries/ignore": [
        "src/test/**/*",
        "src/**/*.test.{ts,tsx}",
        "src/styles/**/*",
        "src/vite-env.d.ts",
      ],
      "boundaries/elements": [
        { type: "domain", mode: "full", pattern: "src/domain/**/*" },
        { type: "ports", mode: "full", pattern: "src/ports/**/*" },
        { type: "application", mode: "full", pattern: "src/application/**/*" },
        {
          type: "infrastructure",
          mode: "full",
          pattern: "src/infrastructure/**/*",
        },
        {
          type: "app",
          mode: "full",
          pattern: ["src/app/**/*", "src/main.tsx"],
        },
        { type: "views", mode: "full", pattern: "src/views/**/*" },
      ],
    },
    rules: {
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "warn",
      "react-refresh/only-export-components": [
        "warn",
        { allowConstantExport: true },
      ],
      "boundaries/element-types": [
        "error",
        {
          default: "disallow",
          rules: [
            { from: ["domain"], allow: ["domain"] },
            { from: ["ports"], allow: ["ports", "domain"] },
            {
              from: ["application"],
              allow: ["application", "domain", "ports"],
            },
            {
              from: ["infrastructure"],
              allow: ["infrastructure", "domain", "ports"],
            },
            { from: ["views"], allow: ["views", "domain", "application"] },
            {
              from: ["app"],
              allow: [
                "app",
                "domain",
                "ports",
                "application",
                "infrastructure",
                "views",
              ],
            },
          ],
        },
      ],
    },
  },
);
