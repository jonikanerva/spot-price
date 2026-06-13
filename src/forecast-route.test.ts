import { afterEach, describe, expect, it } from "vitest";
import type { Pool } from "pg";
import { closeDatabase, initTestDatabase } from "./db.js";
import { createTestApp } from "./test-utils.js";
import { storeFingridRecords } from "./fingrid-store.js";
import {
  DATASET_CONSUMPTION_ACTUAL,
  DATASET_CONSUMPTION_FORECAST,
  DATASET_WIND_ACTUAL,
  DATASET_WIND_FORECAST,
} from "./fingrid.js";
import { ForecastResponseSchema } from "./api-schemas.js";
import type { FingridRecord } from "./types.js";

const TEST_USER_ID = "user-fc-1";
const TEST_API_KEY = "sp_test_forecast_123";
const QUARTER_MS = 15 * 60 * 1000;
const DAY_MS = 24 * 60 * 60 * 1000;

const seedUserAndKey = async (pool: Pool): Promise<void> => {
  await pool.query(
    `INSERT INTO user_settings (
      user_id, margin_cents_kwh, transfer_day_cents_kwh, transfer_night_cents_kwh,
      tax_cents_kwh, vat_percent, night_start_hour, night_end_hour, timezone, area
    ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
    ON CONFLICT (user_id) DO NOTHING`,
    [
      TEST_USER_ID,
      0.49,
      2.92,
      1.37,
      2.82752,
      25.5,
      22,
      7,
      "Europe/Helsinki",
      "FI",
    ],
  );
  await pool.query(
    `INSERT INTO api_keys (id, user_id, key_plaintext) VALUES ($1, $2, $3)
     ON CONFLICT (user_id) DO NOTHING`,
    ["key-fc-1", TEST_USER_ID, TEST_API_KEY],
  );
};

/**
 * Seed ~21 days of FI prices ending at the last full quarter before now.
 *
 * One multi-row INSERT rather than 2016 awaited single-row round-trips: the
 * row-by-row version dominated the test at ~5-6 s and tipped these heavy-setup
 * tests over vitest's 5 s default in the cold-DB parallel run. The batched form
 * runs in well under a second, so the test reflects the route, not the seed.
 */
const seedPriceHistory = async (
  pool: Pool,
  anchorMs: number,
): Promise<void> => {
  const startMs = anchorMs - 21 * DAY_MS;
  const tuples: string[] = [];
  const params: (string | number)[] = [];
  let i = 1;
  for (let q = 0; q < 21 * 96; q++) {
    const ms = startMs + q * QUARTER_MS;
    const start = new Date(ms).toISOString();
    const end = new Date(ms + QUARTER_MS).toISOString();
    const hour = new Date(ms).getUTCHours();
    // EUR/MWh: a daily ramp so there is real structure to fit.
    const eurMwh = 20 + hour * 2;
    tuples.push(`($${String(i++)}, $${String(i++)}, $${String(i++)}, 'FI')`);
    params.push(start, end, eurMwh);
  }
  await pool.query(
    `INSERT INTO prices (delivery_start, delivery_end, price_eur_mwh, area)
     VALUES ${tuples.join(", ")}
     ON CONFLICT (delivery_start, area) DO UPDATE
       SET delivery_end = EXCLUDED.delivery_end, price_eur_mwh = EXCLUDED.price_eur_mwh`,
    params,
  );
};

/** Seed Fingrid datasets across the whole [history, forecast] window. */
const seedFingrid = async (pool: Pool, anchorMs: number): Promise<void> => {
  const startMs = anchorMs - 21 * DAY_MS;
  const records: FingridRecord[] = [];
  for (let q = 0; q < 24 * 96; q++) {
    const ms = startMs + q * QUARTER_MS;
    const startTime = new Date(ms).toISOString();
    const endTime = new Date(ms + QUARTER_MS).toISOString();
    const hour = new Date(ms).getUTCHours();
    const consumption = 8000 + hour * 50;
    const wind = 3000 + ((q * 17) % 1000);
    records.push(
      { datasetId: DATASET_WIND_FORECAST, startTime, endTime, value: wind },
      { datasetId: DATASET_WIND_ACTUAL, startTime, endTime, value: wind },
      {
        datasetId: DATASET_CONSUMPTION_FORECAST,
        startTime,
        endTime,
        value: consumption,
      },
      {
        datasetId: DATASET_CONSUMPTION_ACTUAL,
        startTime,
        endTime,
        value: consumption,
      },
    );
  }
  await storeFingridRecords(pool, records);
};

describe("forecast endpoint", () => {
  let pool: Pool;

  afterEach(async () => {
    await closeDatabase(pool);
  });

  it("returns 401 without an API key", async () => {
    pool = await initTestDatabase();
    const app = createTestApp(pool);
    const response = await app.request("/api/v1/price/forecast");
    expect(response.status).toBe(401);
  });

  it("returns available:false degraded for a non-FI area", async () => {
    pool = await initTestDatabase();
    await seedUserAndKey(pool);
    const app = createTestApp(pool);

    const response = await app.request("/api/v1/price/forecast?area=SE3", {
      headers: { Authorization: `Bearer ${TEST_API_KEY}` },
    });
    expect(response.status).toBe(200);
    const body = ForecastResponseSchema.parse(await response.json());
    expect(body.forecast).toBe(true);
    expect(body.available).toBe(false);
    expect(body.degraded).toBe(true);
    expect(body.reason).toBe("Forecast available for FI only");
    expect(body.entries).toHaveLength(0);
  });

  it("returns available:false when no Fingrid data is stored", async () => {
    pool = await initTestDatabase();
    await seedUserAndKey(pool);
    const app = createTestApp(pool);

    const response = await app.request("/api/v1/price/forecast", {
      headers: { Authorization: `Bearer ${TEST_API_KEY}` },
    });
    expect(response.status).toBe(200);
    const body = ForecastResponseSchema.parse(await response.json());
    expect(body.available).toBe(false);
    expect(body.degraded).toBe(true);
    expect(body.confidence).toBe("low");
  });

  it("returns a structurally-distinct FI estimate with money fields named estimated*", async () => {
    pool = await initTestDatabase();
    await seedUserAndKey(pool);
    // Anchor at a whole quarter so the last published price is deterministic.
    const anchorMs = Math.floor(Date.now() / QUARTER_MS) * QUARTER_MS;
    await seedPriceHistory(pool, anchorMs);
    await seedFingrid(pool, anchorMs);
    const app = createTestApp(pool);

    const response = await app.request("/api/v1/price/forecast", {
      headers: { Authorization: `Bearer ${TEST_API_KEY}` },
    });
    expect(response.status).toBe(200);
    const raw = (await response.json()) as Record<string, unknown>;
    const body = ForecastResponseSchema.parse(raw);

    expect(body.forecast).toBe(true);
    expect(body.area).toBe("FI");
    expect(body.available).toBe(true);
    expect(body.entries.length).toBeGreaterThan(0);

    const first = body.entries[0];
    expect(first).toBeDefined();
    if (first) {
      expect(first.estimated).toBe(true);
      expect(typeof first.estimatedSpotCentsKwh).toBe("number");
      expect(typeof first.estimatedTotalCentsKwh).toBe("number");
      // Total must exceed spot (margin + transfer + tax + VAT all positive here).
      expect(first.estimatedTotalCentsKwh).toBeGreaterThan(
        first.estimatedSpotCentsKwh,
      );
    }

    // Forecast schema shares no money-field name with the real-price schema:
    // the raw payload must NOT carry spotCentsKwh / totalCentsKwh anywhere.
    const rawEntries = raw["entries"] as Record<string, unknown>[];
    for (const entry of rawEntries) {
      expect(entry["spotCentsKwh"]).toBeUndefined();
      expect(entry["totalCentsKwh"]).toBeUndefined();
    }

    // Phase 2: bands ship DARK in the committed artifact, so the descriptor is
    // present and calibrated:false, and NO entry carries band bound fields.
    expect(body.bands).toBeDefined();
    expect(body.bands?.calibrated).toBe(false);
    expect(body.bands?.method).toBe("empirical-residual");
    expect(body.bands?.observedCoverage).toBeNull();
    for (const entry of rawEntries) {
      expect(entry["estimatedSpotLowCentsKwh"]).toBeUndefined();
      expect(entry["estimatedSpotHighCentsKwh"]).toBeUndefined();
      expect(entry["estimatedTotalLowCentsKwh"]).toBeUndefined();
      expect(entry["estimatedTotalHighCentsKwh"]).toBeUndefined();
    }
  });

  it("anchors the series strictly after the last published price (no overlap)", async () => {
    pool = await initTestDatabase();
    await seedUserAndKey(pool);
    const anchorMs = Math.floor(Date.now() / QUARTER_MS) * QUARTER_MS;
    await seedPriceHistory(pool, anchorMs);
    await seedFingrid(pool, anchorMs);
    const app = createTestApp(pool);

    const { rows } = await pool.query<{ last: string }>(
      `SELECT MAX(delivery_start) AS last FROM prices WHERE area = 'FI'`,
    );
    const lastPublishedMs = new Date(rows[0]?.last ?? "").getTime();

    const response = await app.request("/api/v1/price/forecast", {
      headers: { Authorization: `Bearer ${TEST_API_KEY}` },
    });
    const body = ForecastResponseSchema.parse(await response.json());
    const firstStart = body.entries[0]?.start;
    expect(firstStart).toBeDefined();
    if (firstStart) {
      expect(new Date(firstStart).getTime()).toBe(lastPublishedMs + QUARTER_MS);
    }
  });
});
