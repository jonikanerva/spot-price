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
