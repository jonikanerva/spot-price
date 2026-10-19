import { test, expect, type Page } from "@playwright/test";

import { PADDING_LEFT, readChart } from "./chart.js";

const QUARTER_MS = 15 * 60_000;

// 15-minute FI spot prices from `firstStart` on.
const spotSlots = (firstStart: string, count: number) =>
  Array.from({ length: count }, (_, index) => {
    const start = Date.parse(firstStart) + index * QUARTER_MS;
    return {
      deliveryStart: new Date(start).toISOString(),
      deliveryEnd: new Date(start + QUARTER_MS).toISOString(),
      priceEurMwh: 50 + (index % 7),
      area: "FI",
      spotCentsKwh: 5 + (index % 7) / 10,
    };
  });

// `tomorrowComplete` is the service rule of `tomorrowAvailable`: true only
// when `tomorrow` has every quarter of the next local day.
const showPublicSpot = async (
  page: Page,
  today: ReturnType<typeof spotSlots>,
  tomorrow: ReturnType<typeof spotSlots>,
  tomorrowComplete = false,
) => {
  await page.route("**/api/public/spot", (route) =>
    route.fulfill({
      json: {
        area: "FI",
        today,
        tomorrow,
        tomorrowAvailable: tomorrowComplete,
        unit: "c/kWh",
        resolutionMinutes: 15,
      },
    }),
  );
  await page.goto("/");
  await expect(page.locator("#publicChart path")).toHaveCount(
    tomorrow.length > 0 ? 2 : 1,
  );
};

// The x position of a time that is `quarters` slots after the first slot,
// on an axis of `total` slots.
const xAt = (chartWidth: number, quarters: number, total: number) =>
  PADDING_LEFT + (quarters / total) * chartWidth;

test.describe("Public chart time axis", () => {
  // A browser zone far from the zone of the data (Europe/Helsinki).
  test.use({ timezoneId: "America/New_York" });

  test("places the first hour of tomorrow after the midnight tick and joins the line there", async ({
    page,
  }) => {
    // 2026-10-18 in Helsinki, and the one hour of 2026-10-19 that the
    // day-ahead auction of 2026-10-18 already covers.
    await showPublicSpot(
      page,
      spotSlots("2026-10-17T21:00:00.000Z", 96),
      spotSlots("2026-10-18T21:00:00.000Z", 4),
    );

    const chart = await readChart(page, "publicChart");
    expect(chart.ticks.map((tick) => tick.label)).toEqual([
      "00",
      "06",
      "12",
      "18",
      "00",
    ]);
    [0, 24, 48, 72, 96].forEach((quarters, index) => {
      expect(chart.ticks[index]?.x).toBeCloseTo(
        xAt(chart.chartWidth, quarters, 100),
        0,
      );
    });

    const [today = [], tomorrow = []] = chart.paths;
    expect(today).toHaveLength(97);
    expect(tomorrow).toHaveLength(4);
    expect(tomorrow[0]).toEqual(today[today.length - 1]);
    expect(tomorrow[0]?.[0]).toBeCloseTo(xAt(chart.chartWidth, 96, 100), 0);

    const box = await page.locator("#publicChart").boundingBox();
    if (!box) throw new Error("the public chart has no box");
    await page.mouse.move(box.x + PADDING_LEFT + 1, box.y + box.height / 2);
    await expect(page.locator("#publicTooltip")).toHaveText(/^00:00 - /);
    await page.mouse.move(
      box.x + PADDING_LEFT + chart.chartWidth - 1,
      box.y + box.height / 2,
    );
    await expect(page.locator("#publicTooltip")).toHaveText(/^00:45 - /);
  });

  test("spaces the ticks by local wall time on a 25-hour day", async ({
    page,
  }) => {
    // 2026-10-25 in Helsinki: 03:00 EEST becomes 03:00 EET, so 06:00 is
    // 7 hours after midnight.
    await showPublicSpot(page, spotSlots("2026-10-24T21:00:00.000Z", 100), []);

    const chart = await readChart(page, "publicChart");
    expect(chart.ticks.map((tick) => tick.label)).toEqual([
      "00",
      "06",
      "12",
      "18",
      "00",
    ]);
    [0, 28, 52, 76, 100].forEach((quarters, index) => {
      expect(chart.ticks[index]?.x).toBeCloseTo(
        xAt(chart.chartWidth, quarters, 100),
        0,
      );
    });
  });
});
