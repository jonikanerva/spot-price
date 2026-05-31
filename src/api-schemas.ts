import { z } from "@hono/zod-openapi";
import { DELIVERY_AREAS, SUPPORTED_TIMEZONES } from "./areas.js";

// ---------------------------------------------------------------------------
// Shared area code enum derived from DELIVERY_AREAS
// ---------------------------------------------------------------------------

const areaCodes = DELIVERY_AREAS.map((a) => a.code) as [string, ...string[]];
const timezoneValues = [...SUPPORTED_TIMEZONES] as [string, ...string[]];

// ---------------------------------------------------------------------------
// Response schemas
// ---------------------------------------------------------------------------

export const TotalPriceSchema = z
  .object({
    deliveryStart: z.string().openapi({ example: "2026-02-27T10:00:00.000Z" }),
    deliveryEnd: z.string().openapi({ example: "2026-02-27T10:15:00.000Z" }),
    localStart: z
      .string()
      .openapi({ example: "2026-02-27T12:00:00.000+02:00" }),
    localEnd: z.string().openapi({ example: "2026-02-27T12:15:00.000+02:00" }),
    spotCentsKwh: z.number().openapi({ example: 5.23 }),
    marginCentsKwh: z.number().openapi({ example: 0.5 }),
    transferCentsKwh: z.number().openapi({ example: 2.5 }),
    taxCentsKwh: z.number().openapi({ example: 2.79372 }),
    vatCentsKwh: z.number().openapi({ example: 2.81 }),
    totalCentsKwh: z.number().openapi({ example: 13.83 }),
    isNightRate: z.boolean().openapi({ example: false }),
  })
  .openapi("TotalPrice");

export const PriceWindowSchema = z
  .object({
    start: z.string().openapi({ example: "2026-02-27T22:00:00.000Z" }),
    end: z.string().openapi({ example: "2026-02-28T01:00:00.000Z" }),
    startLocal: z
      .string()
      .openapi({ example: "2026-02-28T00:00:00.000+02:00" }),
    endLocal: z.string().openapi({ example: "2026-02-28T03:00:00.000+02:00" }),
    minTotalCentsKwh: z.number().openapi({ example: 6.21 }),
    maxTotalCentsKwh: z.number().openapi({ example: 11.05 }),
    averageTotalCentsKwh: z.number().openapi({ example: 8.42 }),
    prices: z.array(TotalPriceSchema),
  })
  .openapi("PriceWindow");

export const PriceListSchema = PriceWindowSchema.extend({
  available: z.boolean().openapi({ example: true }),
  expectedAt: z.string().optional().openapi({ example: "12:00 UTC" }),
}).openapi("PriceList");

const SpotPriceEntrySchema = z
  .object({
    deliveryStart: z.string().openapi({ example: "2026-02-27T10:00:00.000Z" }),
    deliveryEnd: z.string().openapi({ example: "2026-02-27T10:15:00.000Z" }),
    priceEurMwh: z.number().openapi({ example: 52.3 }),
    area: z.string().openapi({ example: "FI" }),
    spotCentsKwh: z.number().openapi({ example: 5.23 }),
  })
  .openapi("SpotPriceEntry");

export const PublicSpotSchema = z
  .object({
    area: z.string().openapi({ example: "FI" }),
    today: z.array(SpotPriceEntrySchema),
    tomorrow: z.array(SpotPriceEntrySchema),
    tomorrowAvailable: z.boolean().openapi({ example: true }),
    unit: z.string().openapi({ example: "c/kWh" }),
    resolutionMinutes: z.number().int().openapi({ example: 15 }),
  })
  .openapi("PublicSpot");

export const UserSettingsResponseSchema = z
  .object({
    userId: z.string().openapi({ example: "abc123" }),
    marginCentsKwh: z.number().openapi({ example: 0.5 }),
    transferDayCentsKwh: z.number().openapi({ example: 2.5 }),
    transferNightCentsKwh: z.number().openapi({ example: 1.2 }),
    taxCentsKwh: z.number().openapi({ example: 2.79372 }),
    vatPercent: z.number().openapi({ example: 25.5 }),
    nightStartHour: z.number().int().openapi({ example: 22 }),
    nightEndHour: z.number().int().openapi({ example: 7 }),
    timezone: z.string().openapi({ example: "Europe/Helsinki" }),
    area: z.string().openapi({ example: "FI" }),
  })
  .openapi("UserSettings");

export const ChartDataSchema = z
  .object({
    today: z.array(TotalPriceSchema),
    tomorrow: z.array(TotalPriceSchema),
    tomorrowAvailable: z.boolean().openapi({ example: true }),
    unit: z.string().openapi({ example: "c/kWh" }),
    resolutionMinutes: z.number().int().openapi({ example: 15 }),
  })
  .openapi("ChartData");

export const ErrorSchema = z
  .object({
    error: z.string().openapi({ example: "Description of what went wrong" }),
  })
  .openapi("Error");

// ---------------------------------------------------------------------------
// Request schemas
// ---------------------------------------------------------------------------

export const CheapestQuerySchema = z.object({
  duration: z.coerce.number().int().min(1).max(1440).openapi({
    example: 180,
    description: "Window duration in minutes (1-1440)",
  }),
  startTime: z
    .string()
    .transform((s) => s.replace(/ /g, "+"))
    .pipe(z.iso.datetime({ offset: true }))
    .optional()
    .openapi({
      example: "2026-02-27T14:00:00+02:00",
      description: "Earliest allowed window start (ISO 8601)",
    }),
  endTime: z
    .string()
    .transform((s) => s.replace(/ /g, "+"))
    .pipe(z.iso.datetime({ offset: true }))
    .optional()
    .openapi({
      example: "2026-02-28T06:00:00+02:00",
      description: "Latest allowed window end (ISO 8601)",
    }),
  maxPrice: z
    .string()
    .trim()
    .min(1, "maxPrice is required when provided")
    .transform((value) => Number(value))
    .refine(Number.isFinite, "maxPrice must be a valid number")
    .optional()
    .openapi({
      example: 12,
      description:
        "Maximum allowed total price in c/kWh for each interval in the selected window",
    }),
});

/** Inclusive maximum span of a price-history query, in calendar days. */
const MAX_HISTORY_SPAN_DAYS = 31;

/**
 * Count of inclusive calendar days between two YYYY-MM-DD strings.
 *
 * Uses `Date.UTC` on the parsed calendar-label parts (same `Number(str.slice(...))`
 * pattern as `getUtcRangeForLocalDate`), so the count is pure calendar arithmetic
 * and DST-immune — no wall-clock offset ever enters the computation.
 */
const inclusiveDaySpan = (from: string, to: string): number => {
  const fromMs = Date.UTC(
    Number(from.slice(0, 4)),
    Number(from.slice(5, 7)) - 1,
    Number(from.slice(8, 10)),
  );
  const toMs = Date.UTC(
    Number(to.slice(0, 4)),
    Number(to.slice(5, 7)) - 1,
    Number(to.slice(8, 10)),
  );
  return (toMs - fromMs) / 86_400_000 + 1;
};

export const PriceHistoryQuerySchema = z
  .object({
    from: z.iso.date().openapi({
      example: "2026-04-01",
      description:
        "Inclusive range start, YYYY-MM-DD, interpreted in the user's timezone.",
    }),
    to: z.iso.date().openapi({
      example: "2026-05-01",
      description:
        "Inclusive range end, YYYY-MM-DD, interpreted in the user's timezone.",
    }),
  })
  // Ordering is compared lexicographically on the validated YYYY-MM-DD strings
  // (lexicographic === chronological for zero-padded ISO dates), so no Date is
  // constructed for the comparison itself.
  .refine((v) => v.from <= v.to, {
    message: "from must be on or before to",
    path: ["from"],
  })
  .refine((v) => inclusiveDaySpan(v.from, v.to) <= MAX_HISTORY_SPAN_DAYS, {
    message: "date range must not exceed 31 days",
    path: ["to"],
  });

export const SpotQuerySchema = z.object({
  area: z.enum(areaCodes).optional().default("FI").openapi({
    example: "FI",
    description: "Nord Pool delivery area code",
  }),
});

export const UserSettingsUpdateSchema = z.object({
  marginCentsKwh: z.number().min(0).optional().openapi({ example: 0.5 }),
  transferDayCentsKwh: z.number().min(0).optional().openapi({ example: 2.5 }),
  transferNightCentsKwh: z.number().min(0).optional().openapi({ example: 1.2 }),
  taxCentsKwh: z.number().min(0).optional().openapi({ example: 2.79372 }),
  vatPercent: z
    .number()
    .min(0)
    .max(100)
    .optional()
    .openapi({ example: 25.5, description: "VAT percentage (0-100)" }),
  nightStartHour: z
    .number()
    .int()
    .min(0)
    .max(23)
    .optional()
    .openapi({ example: 22, description: "Night rate start hour (0-23)" }),
  nightEndHour: z
    .number()
    .int()
    .min(0)
    .max(23)
    .optional()
    .openapi({ example: 7, description: "Night rate end hour (0-23)" }),
  timezone: z
    .enum(timezoneValues)
    .optional()
    .openapi({ example: "Europe/Helsinki" }),
  area: z.enum(areaCodes).optional().openapi({ example: "FI" }),
});
