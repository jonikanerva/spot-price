import { defineConfig } from "vitest/config";
import { readFileSync, existsSync } from "node:fs";
import { resolve } from "node:path";

const loadDotEnv = (): Record<string, string> => {
  const envPath = resolve(process.cwd(), ".env");
  if (!existsSync(envPath)) return {};
  const content = readFileSync(envPath, "utf-8");
  const env: Record<string, string> = {};
  for (const line of content.split("\n")) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    const eqIndex = trimmed.indexOf("=");
    if (eqIndex === -1) continue;
    const key = trimmed.slice(0, eqIndex).trim();
    let value = trimmed.slice(eqIndex + 1).trim();
    // Strip surrounding quotes (single or double)
    if (
      value.length >= 2 &&
      ((value.startsWith('"') && value.endsWith('"')) ||
        (value.startsWith("'") && value.endsWith("'")))
    ) {
      value = value.slice(1, -1);
    }
    env[key] = value;
  }
  return env;
};

export default defineConfig({
  test: {
    globals: false,
    environment: "node",
    // src/ holds the shipped code and the conformal tests; tools/ holds the
    // offline backtest engine/CLI/regen and their tests. Both globs run so
    // moving a test to tools/ never silently drops it from the suite.
    include: ["src/**/*.test.ts", "tools/**/*.test.ts"],
    env: loadDotEnv(),
  },
});
