import { test, expect } from "@playwright/test";

const TEST_USER = `e2e_${Date.now()}`;
const TEST_PASS = "testpass1234";

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
    // Wait for chart to attempt rendering (even if no data, SVG should exist)
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

    // Wait for dashboard to appear
    await expect(page.locator("#dashboard")).toBeVisible({ timeout: 10_000 });
    await expect(page.locator("#usernameLabel")).toContainText(TEST_USER);
  });

  test("logout returns to landing", async ({ page }) => {
    await page.goto("/");

    // Login first
    await page.fill("#username", `e2e_logout_${Date.now()}`);
    await page.fill("#password", TEST_PASS);
    await page.click("#loginBtn");
    await expect(page.locator("#dashboard")).toBeVisible({ timeout: 10_000 });

    // Logout
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

  test("total chart SVG is present", async ({ page }) => {
    const svg = page.locator("#totalChart");
    await expect(svg).toBeVisible();
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
    // Wait for key to load (starts with sp_)
    await expect(keyDisplay).toContainText("sp_", { timeout: 5_000 });
  });

  test("regenerate creates a new key", async ({ page }) => {
    await page.click("#navApi");
    await expect(page.locator("#apiKeyDisplay")).toContainText("sp_", {
      timeout: 5_000,
    });

    const oldKey = await page.locator("#apiKeyDisplay").textContent();

    await page.click("#regenBtn");
    // Wait for status message confirming regeneration
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
