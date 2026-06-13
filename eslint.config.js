import eslint from "@eslint/js";
import tseslint from "typescript-eslint";

export default tseslint.config(
  eslint.configs.recommended,
  ...tseslint.configs.strictTypeChecked,
  {
    languageOptions: {
      parserOptions: {
        // Type-aware linting via the TypeScript project service. It discovers
        // the nearest tsconfig for each file: src/ -> tsconfig.json, tools/ ->
        // tools/tsconfig.json. Both are kept in sync with `pnpm typecheck`.
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
      },
    },
    rules: {
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_" },
      ],
      "@typescript-eslint/no-confusing-void-expression": "off",
    },
  },
  {
    // Offline-only guarantee, layer 3 of 4 (directory / build-root / LINT /
    // tree-shaking): src/ runtime code must never import tools/. tools/ holds
    // the offline backtest engine, CLI, and band regen; importing any of it from
    // a shipped module would drag offline-only code toward the production
    // bundle. Tests are exempt (they legitimately import the engine to test it).
    files: ["src/**/*.ts"],
    ignores: ["src/**/*.test.ts"],
    rules: {
      "no-restricted-imports": [
        "error",
        {
          patterns: [
            {
              group: ["**/tools/**", "../tools/*", "../../tools/*"],
              message:
                "src/ runtime must not import tools/ (offline dev-only; must never reach the production bundle).",
            },
          ],
        },
      ],
    },
  },
  {
    ignores: [
      "dist/",
      "node_modules/",
      ".opencode/",
      ".worktrees/",
      "e2e/",
      "*.config.js",
      "*.config.ts",
      "scripts/",
    ],
  },
);
