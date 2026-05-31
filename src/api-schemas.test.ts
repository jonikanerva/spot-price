import { describe, it, expect } from "vitest";
import { PriceHistoryQuerySchema } from "./api-schemas.js";

describe("PriceHistoryQuerySchema", () => {
  const parse = (from: string, to: string): boolean =>
    PriceHistoryQuerySchema.safeParse({ from, to }).success;

  it("accepts a valid date-only range", () => {
    expect(parse("2026-04-01", "2026-04-15")).toBe(true);
  });

  it("accepts a single-day range (from === to)", () => {
    expect(parse("2026-04-01", "2026-04-01")).toBe(true);
  });

  it("rejects from after to", () => {
    expect(parse("2026-05-02", "2026-05-01")).toBe(false);
  });

  it("surfaces the ordering message with the from path", () => {
    const result = PriceHistoryQuerySchema.safeParse({
      from: "2026-05-02",
      to: "2026-05-01",
    });
    expect(result.success).toBe(false);
    if (!result.success) {
      const issue = result.error.issues.find(
        (i) => i.message === "from must be on or before to",
      );
      expect(issue?.path).toEqual(["from"]);
    }
  });

  it("accepts exactly 30 inclusive days", () => {
    // 2026-04-01 .. 2026-04-30 = 30 inclusive days
    expect(parse("2026-04-01", "2026-04-30")).toBe(true);
  });

  it("accepts exactly 31 inclusive days", () => {
    // 2026-04-01 .. 2026-05-01 = 31 inclusive days
    expect(parse("2026-04-01", "2026-05-01")).toBe(true);
  });

  it("rejects 32 inclusive days", () => {
    // 2026-04-01 .. 2026-05-02 = 32 inclusive days
    expect(parse("2026-04-01", "2026-05-02")).toBe(false);
  });

  it("surfaces the span message with the to path", () => {
    const result = PriceHistoryQuerySchema.safeParse({
      from: "2026-04-01",
      to: "2026-05-02",
    });
    expect(result.success).toBe(false);
    if (!result.success) {
      const issue = result.error.issues.find(
        (i) => i.message === "date range must not exceed 31 days",
      );
      expect(issue?.path).toEqual(["to"]);
    }
  });

  it("rejects a datetime value (date-only required)", () => {
    expect(parse("2026-04-01T00:00:00Z", "2026-04-15")).toBe(false);
    expect(parse("2026-04-01", "2026-04-15T12:00:00+02:00")).toBe(false);
  });

  it("rejects garbage and impossible dates", () => {
    expect(parse("not-a-date", "2026-04-15")).toBe(false);
    expect(parse("2026-13-40", "2026-05-01")).toBe(false);
    expect(parse("2026-04-01", "")).toBe(false);
  });

  it("rejects a missing field", () => {
    expect(
      PriceHistoryQuerySchema.safeParse({ from: "2026-04-01" }).success,
    ).toBe(false);
  });
});
