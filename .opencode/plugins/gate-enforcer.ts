import { execSync } from "node:child_process";

const hasCodeReviewComment = (prNumber: string): boolean => {
  try {
    const raw = execSync(
      `gh pr view ${prNumber} --json comments --jq '.comments[].body'`,
      { encoding: "utf-8", timeout: 10_000 },
    );
    return /\*\*LGTM\*\*|Verdict.*LGTM/.test(raw);
  } catch {
    return false;
  }
};

const extractPrNumber = (cmd: string): string | null => {
  const match = cmd.match(/gh\s+pr\s+merge\s+(\d+)/);
  return match ? match[1] : null;
};

const getCurrentBranch = (): string => {
  try {
    return execSync("git branch --show-current", {
      encoding: "utf-8",
      timeout: 5_000,
    }).trim();
  } catch {
    return "";
  }
};

const isProtectedBranch = (branch: string): boolean =>
  branch === "main" || branch === "master";

export const GateEnforcer = async () => ({
  "tool.execute.before": async (input: any, output: any) => {
    if (input.tool === "bash") {
      const cmd = String(output.args?.command || "");

      // Block reading .env files
      if (/\b(cat|grep|rg|find)\b.*\.env(\.|\b)/.test(cmd)) {
        throw new Error("Blocked: reading .env content is not allowed");
      }

      // Block force push and direct push to protected branches
      if (cmd.startsWith("git push")) {
        if (/\s--force(\s|$)|\s--force-with-lease(\s|$)/.test(cmd)) {
          throw new Error("Blocked: force push is not allowed");
        }
        if (/\borigin\s+(main|master)(\s|$)/.test(cmd)) {
          throw new Error(
            "Blocked: pushing directly to protected branch is not allowed",
          );
        }
      }

      // Block destructive commands
      if (cmd.includes("rm -rf"))
        throw new Error("Blocked: destructive command");

      // Block commits on protected branches
      if (/\bgit\s+commit\b/.test(cmd)) {
        const branch = getCurrentBranch();
        if (isProtectedBranch(branch)) {
          throw new Error(
            `Blocked: committing directly to '${branch}' is not allowed. ` +
              "Create a feature branch first.",
          );
        }
      }

      // Block local merges into protected branches
      if (/\bgit\s+merge\b/.test(cmd)) {
        const branch = getCurrentBranch();
        if (isProtectedBranch(branch)) {
          throw new Error(
            `Blocked: merging into '${branch}' locally is not allowed. ` +
              "Use a pull request with code review instead.",
          );
        }
      }

      // Block PR merge without code review
      const prNumber = extractPrNumber(cmd);
      if (prNumber && !hasCodeReviewComment(prNumber)) {
        throw new Error(
          `Blocked: PR #${prNumber} has no code review comment with LGTM verdict. ` +
            "Run the code-reviewer agent before merging.",
        );
      }
    }
  },
});
