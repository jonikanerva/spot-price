import { z } from "zod";
import type { FingridFetchResult, FingridRecord } from "./types.js";

/**
 * Fingrid Open Data fetch boundary.
 *
 * Calls the Fingrid Open Data multi-dataset endpoint for the public Finnish
 * grid series the forecast needs, validates the response with zod (STACK.md
 * forbids raw `fetch` without zod-validated parsing), and degrades gracefully:
 * a timeout, auth error, HTTP error, or malformed body yields an empty
 * `records` array plus a `reason` — it NEVER throws, so a Fingrid problem can
 * never break the authoritative Nord Pool price path.
 *
 * The API key is passed in as a parameter so this module does not touch
 * `process.env` / `env.ts` — the boundary stays a pure function of (key,
 * range, datasets), which also keeps it trivial to leave un-exercised in tests
 * that have no key.
 */

const BASE_URL = "https://data.fingrid.fi/api/data";

/** Fingrid dataset ids used by the forecast. */
export const DATASET_WIND_FORECAST = 245; // wind power forecast, 15 min, ~72h
export const DATASET_WIND_ACTUAL = 75; // actual wind power, 15 min
export const DATASET_CONSUMPTION_FORECAST = 165; // consumption forecast, 15 min, ~24h
export const DATASET_CONSUMPTION_ACTUAL = 124; // actual consumption (hourly historically; 15-min since 2025 MTU)

/** All four datasets, in one request. */
export const FORECAST_DATASETS: readonly number[] = [
  DATASET_WIND_FORECAST,
  DATASET_WIND_ACTUAL,
  DATASET_CONSUMPTION_FORECAST,
  DATASET_CONSUMPTION_ACTUAL,
];

const REQUEST_TIMEOUT_MS = 30_000;
const MAX_PAGE_SIZE = 20_000;

/** Boundary schema for a single Fingrid observation. */
const FingridRecordSchema = z.object({
  datasetId: z.number(),
  startTime: z.string(),
  endTime: z.string(),
  value: z.number(),
});

/** Boundary schema for the Fingrid response envelope. */
const FingridResponseSchema = z.object({
  data: z.array(FingridRecordSchema),
});

export interface FingridFetchParams {
  readonly apiKey: string;
  /** UTC ISO 8601 inclusive start. */
  readonly startUtc: string;
  /** UTC ISO 8601 exclusive end. */
  readonly endUtc: string;
  /** Dataset ids to request. Defaults to all four forecast datasets. */
  readonly datasets?: readonly number[];
}

const buildUrl = (params: FingridFetchParams): string => {
  const url = new URL(BASE_URL);
  const datasets = params.datasets ?? FORECAST_DATASETS;
  url.searchParams.set("datasets", datasets.join(","));
  url.searchParams.set("startTime", params.startUtc);
  url.searchParams.set("endTime", params.endUtc);
  url.searchParams.set("format", "json");
  url.searchParams.set("pageSize", String(MAX_PAGE_SIZE));
  return url.toString();
};

const degraded = (reason: string): FingridFetchResult => ({
  ok: false,
  records: [],
  reason,
});

/**
 * Fetch the requested Fingrid datasets for a UTC range. Always resolves;
 * failures are reported via the degraded branch of the tagged union and never
 * thrown.
 */
export const fetchFingridSeries = async (
  params: FingridFetchParams,
): Promise<FingridFetchResult> => {
  const url = buildUrl(params);
  const controller = new AbortController();
  const timeout = setTimeout(() => {
    controller.abort();
  }, REQUEST_TIMEOUT_MS);

  try {
    const response = await fetch(url, {
      headers: { "x-api-key": params.apiKey, Accept: "application/json" },
      signal: controller.signal,
    });

    if (response.status === 401 || response.status === 403) {
      return degraded(`Fingrid auth failed (HTTP ${String(response.status)})`);
    }
    if (response.status === 422) {
      return degraded("Fingrid rejected the request (HTTP 422)");
    }
    if (!response.ok) {
      return degraded(`Fingrid returned HTTP ${String(response.status)}`);
    }

    const body: unknown = await response.json();
    const parsed = FingridResponseSchema.safeParse(body);
    if (!parsed.success) {
      return degraded("Fingrid response failed schema validation");
    }

    const records: FingridRecord[] = parsed.data.data.map((r) => ({
      datasetId: r.datasetId,
      startTime: r.startTime,
      endTime: r.endTime,
      value: r.value,
    }));
    return { ok: true, records };
  } catch (error) {
    if (error instanceof Error && error.name === "AbortError") {
      return degraded("Fingrid request timed out");
    }
    const msg = error instanceof Error ? error.message : "unknown error";
    return degraded(`Fingrid request failed: ${msg}`);
  } finally {
    clearTimeout(timeout);
  }
};
