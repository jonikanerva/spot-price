import { z } from "zod";
import type { HourlyPrice } from "./types.js";

const BASE_URL = "https://dataportal-api.nordpoolgroup.com/api/DayAheadPrices";

interface FetchPricesParams {
  readonly date: string; // YYYY-MM-DD
  readonly areas: readonly string[]; // e.g. ["FI"] or ["FI", "SE1", "SE2", ...]
}

/** Only the fields parseResponse consumes are validated; Nord Pool may add
 *  cosmetic envelope fields (deliveryDateCET, updatedAt, currency, areaStates)
 *  and zod strips unknown keys, so additive upstream changes do not degrade us. */
const NordPoolEntrySchema = z.object({
  deliveryStart: z.string(),
  deliveryEnd: z.string(),
  entryPerArea: z.record(z.string(), z.number()), // zod v4: explicit key schema required
});

/** Loose envelope: confirm multiAreaEntries is an array of unknown; each entry
 *  is validated per-item in parseResponse so one bad entry (e.g. a minor area)
 *  cannot zero out the good ones (incl. primary FI). */
const NordPoolResponseSchema = z.object({
  multiAreaEntries: z.array(z.unknown()),
});

type NordPoolResponse = z.infer<typeof NordPoolResponseSchema>;

/** Convert EUR/MWh to c/kWh (divide by 10) */
export const eurMwhToCentsKwh = (eurMwh: number): number =>
  Math.round((eurMwh / 10) * 1000) / 1000;

const buildUrl = (params: FetchPricesParams): string => {
  const url = new URL(BASE_URL);
  url.searchParams.set("date", params.date);
  url.searchParams.set("market", "DayAhead");
  url.searchParams.set("deliveryArea", params.areas.join(","));
  url.searchParams.set("currency", "EUR");
  return url.toString();
};

const parseResponse = (
  data: NordPoolResponse,
  areas: readonly string[],
): readonly HourlyPrice[] => {
  const results: HourlyPrice[] = [];
  for (const raw of data.multiAreaEntries) {
    const entry = NordPoolEntrySchema.safeParse(raw);
    if (!entry.success) {
      continue; // skip one malformed entry; keep the good ones (incl. FI)
    }
    for (const area of areas) {
      const price = entry.data.entryPerArea[area];
      if (price !== undefined) {
        results.push({
          deliveryStart: entry.data.deliveryStart,
          deliveryEnd: entry.data.deliveryEnd,
          priceEurMwh: price,
          area,
        });
      }
    }
  }
  return results;
};

const MAX_RETRIES = 3;
const RETRY_DELAY_MS = 7000;

const delay = (ms: number): Promise<void> =>
  new Promise((resolve) => setTimeout(resolve, ms));

/** HTTP status codes that indicate "no data" rather than a transient error */
const NO_DATA_STATUSES = new Set([404, 204]);

/**
 * Parse the response body into the loose Nord Pool envelope.
 *
 * An empty body is a definite "no data" → returns null so the caller returns []
 * with NO retry. A non-empty body that is invalid JSON (SyntaxError) or whose
 * envelope drifts from the schema (ZodError) THROWS, so the retry budget in
 * fetchDayAheadPrices covers transient upstream corruption before degrading.
 */
const parseJsonBody = async (
  response: Response,
): Promise<NordPoolResponse | null> => {
  const text = await response.text();
  if (text.length === 0) {
    return null; // definite "no data" → caller returns [] with NO retry
  }
  const json: unknown = JSON.parse(text); // throws SyntaxError on invalid JSON → retried
  return NordPoolResponseSchema.parse(json); // throws ZodError on envelope drift → retried
};

/** Fetch day-ahead prices from Nord Pool Data Portal API */
export const fetchDayAheadPrices = async (
  params: FetchPricesParams,
): Promise<readonly HourlyPrice[]> => {
  const url = buildUrl(params);

  for (let attempt = 1; attempt <= MAX_RETRIES; attempt++) {
    try {
      const response = await fetch(url);

      // "No data" responses — return empty array immediately, no retry
      if (NO_DATA_STATUSES.has(response.status)) {
        return [];
      }

      if (!response.ok) {
        throw new Error(
          `Nord Pool API returned ${String(response.status)}: ${response.statusText}`,
        );
      }

      const data = await parseJsonBody(response);
      if (!data) {
        // Empty or unparseable body — no data available, not a transient error
        return [];
      }

      return parseResponse(data, params.areas);
    } catch (error) {
      const isLastAttempt = attempt === MAX_RETRIES;
      if (isLastAttempt) {
        if (error instanceof z.ZodError || error instanceof SyntaxError) {
          // Schema/JSON drift survived the retry budget: degrade to no data
          // with a DISTINCT signal so drift is alertable separately from the
          // benign "not published yet" (which returns [] without this log).
          console.error(
            `[nordpool] NORDPOOL_SCHEMA_DRIFT: upstream body failed validation after ${String(MAX_RETRIES)} attempts, degrading to no data`,
          );
          return []; // degrade, do NOT throw past the cron
        }
        throw error; // genuine HTTP/network error keeps propagating as today
      }
      console.warn(
        `[nordpool] Fetch attempt ${String(attempt)}/${String(MAX_RETRIES)} failed, retrying in ${String(RETRY_DELAY_MS / 1000)}s...`,
      );
      await delay(RETRY_DELAY_MS);
    }
  }

  // Unreachable but satisfies TypeScript
  throw new Error("Max retries exceeded");
};
