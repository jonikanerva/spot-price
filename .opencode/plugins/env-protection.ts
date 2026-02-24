export const EnvProtection = async () => ({
  "tool.execute.before": async (input: any, output: any) => {
    if (input.tool === "read" && String(output.args?.filePath || "").includes(".env")) {
      throw new Error("Blocked: .env reads are not allowed")
    }
  }
})
