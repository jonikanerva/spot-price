export const GateEnforcer = async () => ({
  "tool.execute.before": async (input: any, output: any) => {
    if (input.tool === "bash") {
      const cmd = String(output.args?.command || "")
      if (/\b(cat|grep|rg|find)\b.*\.env(\.|\b)/.test(cmd)) {
        throw new Error("Blocked: reading .env content is not allowed")
      }
      if (cmd.startsWith("git push")) {
        if (/\s--force(\s|$)|\s--force-with-lease(\s|$)/.test(cmd)) {
          throw new Error("Blocked: force push is not allowed")
        }
        if (/\borigin\s+(main|master)(\s|$)/.test(cmd)) {
          throw new Error("Blocked: pushing directly to protected branch is not allowed")
        }
      }
      if (cmd.includes("rm -rf")) throw new Error("Blocked: destructive command")
    }
  }
})
