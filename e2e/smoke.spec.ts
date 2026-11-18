import { test, expect, type Locator } from "@playwright/test";

import { readChart } from "./chart.js";

const TEST_USER = `e2e_${Date.now()}`;
const TEST_PASS = "testpass1234";

// One term cell of a row of the contract terms versions table.
const term = (row: Locator, name: string): Locator =>
  row.locator(`[data-term="${name}"]`);

// YYYY-MM-DD of today in timeZone. A new account has Europe/Helsinki.
const todayIn = (timeZone: string): string =>
  new Intl.DateTimeFormat("en-CA", {
    timeZone,
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  }).format(new Date());

test.describe("Landing page", () => {
  test("loads with title and login form", async ({ page }) => {
    await page.goto("/");
    await expect(page).toHaveTitle(/Spot Price/);
    await expect(page.locator("#loginBtn")).toBeVisible();
    await expect(page.locator("#username")).toBeVisible();
    await expect(page.locator("#password")).toBeVisible();
  });

  test("public chart SVG renders", async ({ page }) => {
    await page.goto("/");
    const svg = page.locator("#publicChart");
    await expect(svg).toBeVisible();
  });
});

test.describe("Auth flow", () => {
  test("login or signup creates account and shows dashboard", async ({
    page,
  }) => {
    await page.goto("/");

    await page.fill("#username", TEST_USER);
    await page.fill("#password", TEST_PASS);
    await page.click("#loginBtn");

    await expect(page.locator("#dashboard")).toBeVisible({ timeout: 10_000 });
    await expect(page.locator("#usernameLabel")).toContainText(TEST_USER);
  });

  test("logout returns to landing", async ({ page }) => {
    await page.goto("/");

    await page.fill("#username", `e2e_logout_${Date.now()}`);
    await page.fill("#password", TEST_PASS);
    await page.click("#loginBtn");
    await expect(page.locator("#dashboard")).toBeVisible({ timeout: 10_000 });

    await page.click("#logoutBtn");
    await expect(page.locator("#landing")).toBeVisible({ timeout: 5_000 });
  });
});

test.describe("Dashboard", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await page.fill("#username", `e2e_dash_${Date.now()}`);
    await page.fill("#password", TEST_PASS);
    await page.click("#loginBtn");
    await expect(page.locator("#dashboard")).toBeVisible({ timeout: 10_000 });
  });

  test("settings panel loads with input fields", async ({ page }) => {
    await expect(page.locator("#margin")).toBeVisible();
    await expect(page.locator("#vat")).toBeVisible();
    await expect(page.locator("#saveBtn")).toBeVisible();
  });

  test("save settings shows success status", async ({ page }) => {
    await page.fill("#margin", "0.5");
    await page.click("#saveBtn");
    await expect(page.locator("#settingsStatus")).toContainText("saved", {
      timeout: 5_000,
    });
  });

  test.describe("in a browser zone far from the settings zone", () => {
    test.use({ timezoneId: "Pacific/Auckland" });

    test("total chart draws today and tomorrow on one local time axis", async ({
      page,
    }) => {
      const svg = page.locator("#totalChart");
      await expect(svg).toBeVisible();
      await expect(svg.locator("path")).toHaveCount(2);

      // The seed covers both local days of the default zone in full.
      const chart = await readChart(page, "totalChart");
      expect(chart.ticks.map((tick) => tick.label)).toEqual([
        "00",
        "06",
        "12",
        "18",
        "00",
        "06",
        "12",
        "18",
        "00",
      ]);
      const [today = [], tomorrow = []] = chart.paths;
      expect(tomorrow[0]).toEqual(today[today.length - 1]);
    });
  });
});

test.describe("Contract terms versions", () => {
  // The login limiter allows 10 sign-ins for each client address in 15
  // minutes. The other journeys use them all, so this one is another client.
  test.use({ extraHTTPHeaders: { "X-Real-IP": "192.0.2.10" } });

  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await page.fill("#username", `e2e_terms_${Date.now()}`);
    await page.fill("#password", TEST_PASS);
    await page.click("#loginBtn");
    await expect(page.locator("#dashboard")).toBeVisible({ timeout: 10_000 });
  });

  test("the settings form has room at desktop width", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 900 });
    const top = async (selector: string): Promise<number> => {
      const box = await page.locator(selector).boundingBox();
      if (!box) throw new Error(`${selector} has no box`);
      return box.y;
    };
    await expect(page.locator("#versionList .version-item")).toHaveCount(1);

    // The eight term fields use two rows of four fields.
    const rows = new Set<number>();
    for (const id of [
      "margin",
      "dayTransfer",
      "nightTransfer",
      "tax",
      "vat",
      "nightStart",
      "nightEnd",
      "validFrom",
    ]) {
      rows.add(Math.round(await top(`#${id}`)));
    }
    expect(rows.size).toBe(2);
    expect(await top("#margin")).toBe(await top("#tax"));
    expect(await top("#vat")).toBe(await top("#validFrom"));
    // Valid from is the last field of the last row.
    const left = async (selector: string): Promise<number> =>
      (await page.locator(selector).boundingBox())?.x ?? -1;
    expect(await left("#validFrom")).toBeGreaterThan(await left("#nightEnd"));

    // No label wraps: each label has the height of the first label.
    const heights = await page
      .locator("#dashView .settings-grid label")
      .evaluateAll((labels) =>
        labels.map((label) => Math.round(label.getBoundingClientRect().height)),
      );
    expect(heights).toHaveLength(10);
    expect(new Set(heights).size).toBe(1);

    // The order is settings, chart, versions. Nothing scrolls sideways.
    expect(await top("#saveBtn")).toBeLessThan(await top("#totalChart"));
    expect(await top("#totalChart")).toBeLessThan(await top("#versionList"));
    const overflow = await page.evaluate(
      () => document.documentElement.scrollWidth - window.innerWidth,
    );
    expect(overflow).toBeLessThanOrEqual(0);

    // On a phone the versions table scrolls inside its card, not the page.
    await page.setViewportSize({ width: 375, height: 812 });
    const narrow = await page.evaluate(() => ({
      page: document.documentElement.scrollWidth - window.innerWidth,
      table:
        (document.querySelector(".version-table-wrap")?.scrollWidth ?? 0) -
        (document.querySelector(".version-table-wrap")?.clientWidth ?? 0),
    }));
    expect(narrow.page).toBeLessThanOrEqual(0);
    expect(narrow.table).toBeGreaterThan(0);
  });

  test("schedule, edit, and delete a future change", async ({ page }) => {
    const items = page.locator("#versionList .version-item");
    const future = page.locator(
      '#versionList .version-item[data-valid-from="2099-01-01"]',
    );

    // A new account has one version. It applies today and cannot be deleted.
    await expect(items).toHaveCount(1);
    await expect(items.first().getByRole("rowheader")).toHaveText(
      "1970-01-01 (applies today)",
    );
    await expect(
      items.first().getByRole("button", { name: /^Delete/ }),
    ).toBeDisabled();
    // The date field starts at today in the settings zone.
    await expect(page.locator("#validFrom")).toHaveValue(/^\d{4}-\d{2}-\d{2}$/);
    await expect(page.locator("#validFrom")).not.toHaveValue("1970-01-01");

    // A save without a date is refused in the page and sends no request.
    const saves: string[] = [];
    page.on("request", (request) => {
      if (request.method() === "PUT") saves.push(request.url());
    });
    await page.fill("#validFrom", "");
    await page.fill("#margin", "0.66");
    await page.click("#saveBtn");
    await expect(page.locator("#settingsStatus")).toHaveText(
      "Valid from date required",
    );
    await expect(page.locator("#saveBtn")).toBeEnabled();
    expect(saves).toEqual([]);
    await expect(items).toHaveCount(1);
    await expect(term(items.first(), "margin")).toHaveText("0.49");

    // Schedule a change: the date first, because a new date loads its terms.
    await page.fill("#validFrom", "2099-01-01");
    await page.fill("#margin", "0.77");
    await page.click("#saveBtn");
    await expect(page.locator("#settingsStatus")).toContainText(
      "These terms apply from 2099-01-01",
    );
    await expect(items).toHaveCount(2);
    await expect(term(future, "margin")).toHaveText("0.77");
    await expect(future).not.toContainText("applies today");
    await expect(term(items.first(), "margin")).toHaveText("0.49");

    // Edit the version with the keyboard only; the save replaces it.
    await page.fill("#margin", "0.1");
    await page.fill("#validFrom", "2000-01-01");
    await future.getByRole("button", { name: /^Edit/ }).focus();
    await page.keyboard.press("Enter");
    await expect(page.locator("#validFrom")).toHaveValue("2099-01-01");
    await expect(page.locator("#margin")).toHaveValue("0.77");
    await expect(page.locator("#margin")).toBeFocused();
    await page.keyboard.press("ControlOrMeta+a");
    await page.keyboard.type("0.88");
    await page.locator("#saveBtn").focus();
    await page.keyboard.press("Enter");
    await expect(term(future, "margin")).toHaveText("0.88");
    await expect(items).toHaveCount(2);

    // A new date loads the terms that apply on it. The form holds the terms
    // of today after a reload, so a save without that load would carry them.
    await page.reload();
    await expect(items).toHaveCount(2);
    await expect(page.locator("#margin")).toHaveValue("0.49");
    await page.fill("#validFrom", "2099-06-01");
    await expect(page.locator("#margin")).toHaveValue("0.88");
    await expect(page.locator("#settingsStatus")).toContainText(
      "Loaded the terms that apply on 2099-06-01",
    );
    await page.fill("#tax", "3");
    await page.click("#saveBtn");
    const later = page.locator(
      '#versionList .version-item[data-valid-from="2099-06-01"]',
    );
    await expect(term(later, "margin")).toHaveText("0.88");
    await expect(term(later, "tax")).toHaveText("3");
    await expect(items).toHaveCount(3);
    // A date before every later version loads the first version again.
    await page.fill("#validFrom", "2050-01-01");
    await expect(page.locator("#margin")).toHaveValue("0.49");
    await expect(page.locator("#tax")).toHaveValue("2.9179");

    // Delete a version with the keyboard only, while the form holds it.
    await later.getByRole("button", { name: /^Edit/ }).click();
    await expect(page.locator("#validFrom")).toHaveValue("2099-06-01");
    await expect(page.locator("#tax")).toHaveValue("3");
    page.once("dialog", (dialog) => void dialog.accept());
    await later.getByRole("button", { name: /^Delete/ }).focus();
    await page.keyboard.press("Enter");
    await expect(page.locator("#versionStatus")).toContainText(
      "Deleted the version from 2099-06-01",
    );
    await expect(items).toHaveCount(2);
    // The form then holds the terms of today and the date of today in the
    // settings zone, so a save does not create the deleted version again.
    await expect(page.locator("#validFrom")).toHaveValue(
      todayIn("Europe/Helsinki"),
    );
    await expect(page.locator("#margin")).toHaveValue("0.49");
    await expect(page.locator("#tax")).toHaveValue("2.9179");
    await expect(page.locator("#settingsStatus")).toHaveText(
      "Loaded the terms that apply today.",
    );

    // Delete the other version; the first version is then the only one again.
    page.once("dialog", (dialog) => void dialog.accept());
    await future.getByRole("button", { name: /^Delete/ }).click();
    await expect(page.locator("#versionStatus")).toContainText(
      "Deleted the version from 2099-01-01",
    );
    await expect(items).toHaveCount(1);
    await expect(
      items.first().getByRole("button", { name: /^Delete/ }),
    ).toBeDisabled();

    // The versions come from the server: they survive a reload.
    await page.reload();
    await expect(items).toHaveCount(1);
    await expect(items.first().getByRole("rowheader")).toContainText(
      "1970-01-01",
    );
  });

  test("invoice prices with VAT give the price of the invoices", async ({
    page,
  }) => {
    // The unit prices of an electricity invoice and a grid invoice, c/kWh
    // with 25.5 % VAT. The tax is 2.8112 + 0.1067.
    const invoice = {
      margin: "0.5",
      dayTransfer: "2.96",
      nightTransfer: "1.56",
      tax: "2.9179",
    };
    const first = page.locator(
      '#versionList .version-item[data-valid-from="1970-01-01"]',
    );
    await expect(first).toHaveCount(1);

    // The date first, because a date change loads the terms of that date.
    await page.fill("#validFrom", "1970-01-01");
    for (const [id, value] of Object.entries(invoice)) {
      await page.fill(`#${id}`, value);
    }
    const request = page.waitForRequest(
      (sent) =>
        sent.method() === "PUT" && sent.url().endsWith("/api/v1/me/settings"),
    );
    await page.click("#saveBtn");
    await expect(page.locator("#settingsStatus")).toContainText(
      "These terms apply from 1970-01-01",
    );
    // The page sends the numbers as typed.
    expect((await request).postDataJSON()).toMatchObject({
      marginInclVatCentsKwh: 0.5,
      transferDayInclVatCentsKwh: 2.96,
      transferNightInclVatCentsKwh: 1.56,
      taxInclVatCentsKwh: 2.9179,
      vatPercent: 25.5,
    });

    // The form and the table show the invoice numbers again after a reload.
    await page.reload();
    await expect(first).toHaveCount(1);
    for (const [id, value] of Object.entries(invoice)) {
      await expect(page.locator(`#${id}`)).toHaveValue(value);
    }
    await expect(term(first, "margin")).toHaveText("0.5");
    await expect(term(first, "transferDay")).toHaveText("2.96");
    await expect(term(first, "transferNight")).toHaveText("1.56");
    await expect(term(first, "tax")).toHaveText("2.9179");

    // The total has VAT on the spot price only: the other prices have it.
    const response = await page.request.get("/api/v1/me/chart");
    expect(response.ok()).toBe(true);
    const chart = (await response.json()) as {
      today: {
        spotCentsKwh: number;
        totalCentsKwh: number;
        isNightRate: boolean;
      }[];
    };
    expect(chart.today.length).toBeGreaterThan(0);
    for (const price of chart.today) {
      const transfer = price.isNightRate ? 1.56 : 2.96;
      const expected =
        Math.max(price.spotCentsKwh, 0) * 1.255 + 0.5 + transfer + 2.9179;
      // The API rounds the total to 3 decimals.
      expect(Math.abs(price.totalCentsKwh - expected)).toBeLessThan(0.00051);
    }
  });
});

test.describe("API panel", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await page.fill("#username", `e2e_api_${Date.now()}`);
    await page.fill("#password", TEST_PASS);
    await page.click("#loginBtn");
    await expect(page.locator("#dashboard")).toBeVisible({ timeout: 10_000 });
  });

  test("navigating to API panel shows API key", async ({ page }) => {
    await page.click("#navApi");
    await expect(page.locator("#apiView")).toBeVisible();

    const keyDisplay = page.locator("#apiKeyDisplay");
    await expect(keyDisplay).toBeVisible();
    await expect(keyDisplay).toContainText("sp_", { timeout: 5_000 });

    await page
      .context()
      .grantPermissions(["clipboard-read", "clipboard-write"], {
        origin: new URL(page.url()).origin,
      });

    const firstBlock = page.locator(".example-block").first();
    const preText = (await firstBlock.locator("pre").textContent()) ?? "";
    const commandOnly = preText.split("\n").slice(1).join("\n").trim();

    await firstBlock.locator(".copy-btn").click();
    const copiedText = await page.evaluate(async () =>
      navigator.clipboard.readText(),
    );

    expect(copiedText).toBe(commandOnly);
    expect(copiedText.startsWith("curl ")).toBe(true);
    expect(copiedText.includes("#")).toBe(false);
  });

  test("regenerate creates a new key", async ({ page }) => {
    await page.click("#navApi");
    await expect(page.locator("#apiKeyDisplay")).toContainText("sp_", {
      timeout: 5_000,
    });

    const oldKey = await page.locator("#apiKeyDisplay").textContent();

    await page.click("#regenBtn");
    await expect(page.locator("#apiStatus")).toContainText("New API key", {
      timeout: 5_000,
    });

    const newKey = await page.locator("#apiKeyDisplay").textContent();
    expect(newKey).not.toBe(oldKey);
    expect(newKey).toContain("sp_");
  });

  test("usage examples contain the API key", async ({ page }) => {
    await page.click("#navApi");
    await expect(page.locator("#apiKeyDisplay")).toContainText("sp_", {
      timeout: 5_000,
    });

    const key = await page.locator("#apiKeyDisplay").textContent();
    const examples = await page.locator("#apiExamples").textContent();
    expect(examples).toContain(key?.trim() ?? "");

    const cheapestExampleText =
      (await page
        .locator(".example-block")
        .filter({
          hasText: "Cheapest 2-hour window today between 08:00-17:00 UTC",
        })
        .locator("pre")
        .textContent()) ?? "";

    expect(cheapestExampleText).toContain("duration=120");
    expect(cheapestExampleText).toContain("startTime=");
    expect(cheapestExampleText).toContain("endTime=");
    expect(cheapestExampleText).not.toContain("%3A");
    expect(cheapestExampleText).not.toContain("$(date");
    expect(cheapestExampleText).toMatch(
      /startTime=\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z/,
    );
    expect(cheapestExampleText).toMatch(
      /endTime=\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z/,
    );
  });

  test("home assistant snippets render with copy buttons", async ({ page }) => {
    await page.click("#navApi");
    await expect(page.locator("#apiKeyDisplay")).toContainText("sp_", {
      timeout: 5_000,
    });

    await expect(page.locator("#haPackagesContent")).toContainText(
      "packages: !include_dir_named packages",
    );
    await expect(page.locator("#haYamlContent")).toContainText("rest_command:");
    await expect(page.locator("#haYamlContent")).toContainText(
      "spot_price_all:",
    );
    await expect(page.locator("#haYamlContent")).toContainText(
      "spot_price_history:",
    );
    await expect(page.locator("#haYamlContent")).toContainText(
      "spot_price_forecast:",
    );
    await expect(page.locator("#haUsageContent")).toContainText(
      "action: rest_command.spot_price_cheapest",
    );

    await expect(page.locator("#copyHaPackagesBtn")).toBeVisible();
    await expect(page.locator("#copyHaYamlBtn")).toBeVisible();
    await expect(page.locator("#copyHaUsageBtn")).toBeVisible();
  });

  test("navigating back to dashboard shows chart", async ({ page }) => {
    await page.click("#navApi");
    await expect(page.locator("#apiView")).toBeVisible();

    await page.click("#navDash");
    await expect(page.locator("#dashView")).toBeVisible();
    await expect(page.locator("#apiView")).not.toBeVisible();
  });
});
