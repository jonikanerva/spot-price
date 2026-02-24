export const AuditTrace = async ({ client }: any) => ({
  event: async ({ event }: any) => {
    if (event.type === "session.status" || event.type === "tool.execute.after") {
      await client.app.log({
        body: {
          service: "agentic-governance",
          level: "info",
          message: "runtime-event",
          extra: { type: event.type }
        }
      })
    }
  }
})
