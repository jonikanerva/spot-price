import type { OpenAPIHono } from "@hono/zod-openapi";
import { createRoute } from "@hono/zod-openapi";
import { calculateTotalPrices } from "../calculator.js";
import { getPricesByRange } from "../price-store.js";
import { ensureUserSettings, upsertUserSettings } from "../user-settings.js";
import { getCurrentAndNextDate, getUtcRangeForLocalDate } from "../time.js";
import type { UserSettings } from "../types.js";
import {
  ChartDataSchema,
  ErrorSchema,
  UserSettingsResponseSchema,
  UserSettingsUpdateSchema,
} from "../api-schemas.js";
import type { AppEnv } from "../app.js";

// ---------------------------------------------------------------------------
// Route definitions
// ---------------------------------------------------------------------------

const settingsGetRoute = createRoute({
  method: "get",
  path: "/api/v1/me/settings",
  tags: ["User"],
  summary: "Get user settings",
  description:
    "Returns the authenticated user's electricity contract settings.",
  responses: {
    200: {
      content: { "application/json": { schema: UserSettingsResponseSchema } },
      description: "Current user settings",
    },
  },
});

const settingsPutRoute = createRoute({
  method: "put",
  path: "/api/v1/me/settings",
  tags: ["User"],
  summary: "Update user settings",
  description:
    "Update one or more electricity contract settings. Only provided fields are changed.",
  request: {
    body: {
      content: { "application/json": { schema: UserSettingsUpdateSchema } },
    },
  },
  responses: {
    200: {
      content: { "application/json": { schema: UserSettingsResponseSchema } },
      description: "Updated user settings",
    },
    400: {
      content: { "application/json": { schema: ErrorSchema } },
      description: "Validation error",
    },
  },
});

const chartRoute = createRoute({
  method: "get",
  path: "/api/v1/me/chart",
  tags: ["User"],
  summary: "Chart data (total prices)",
  description:
    "Returns today's and tomorrow's total prices calculated with the user's contract settings.",
  responses: {
    200: {
      content: { "application/json": { schema: ChartDataSchema } },
      description: "Chart data for today and tomorrow",
    },
  },
});

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

export const registerUserRoutes = (app: OpenAPIHono<AppEnv>): void => {
  app.openapi(settingsGetRoute, (c) => {
    const userId = c.get("sessionUser").id;
    const settings = ensureUserSettings(c.get("db"), userId);
    return c.json(settings);
  });

  app.openapi(settingsPutRoute, (c) => {
    const userId = c.get("sessionUser").id;
    const current = ensureUserSettings(c.get("db"), userId);
    const payload = c.req.valid("json");

    const next: UserSettings = {
      userId,
      marginCentsKwh: payload.marginCentsKwh ?? current.marginCentsKwh,
      transferDayCentsKwh:
        payload.transferDayCentsKwh ?? current.transferDayCentsKwh,
      transferNightCentsKwh:
        payload.transferNightCentsKwh ?? current.transferNightCentsKwh,
      taxCentsKwh: payload.taxCentsKwh ?? current.taxCentsKwh,
      vatPercent: payload.vatPercent ?? current.vatPercent,
      nightStartHour: payload.nightStartHour ?? current.nightStartHour,
      nightEndHour: payload.nightEndHour ?? current.nightEndHour,
      timezone: payload.timezone ?? current.timezone,
      area: payload.area ?? current.area,
    };

    upsertUserSettings(c.get("db"), next);
    return c.json(next, 200);
  });

  app.openapi(chartRoute, (c) => {
    const userId = c.get("sessionUser").id;
    const settings = ensureUserSettings(c.get("db"), userId);

    const tz = settings.timezone;
    const { today, tomorrow } = getCurrentAndNextDate(tz);
    const todayUtc = getUtcRangeForLocalDate(today, tz);
    const tomorrowUtc = getUtcRangeForLocalDate(tomorrow, tz);

    const todaySpot = getPricesByRange(
      c.get("db"),
      todayUtc.startUtc,
      todayUtc.endUtc,
      settings.area,
    );
    const tomorrowSpot = getPricesByRange(
      c.get("db"),
      tomorrowUtc.startUtc,
      tomorrowUtc.endUtc,
      settings.area,
    );

    return c.json({
      today: calculateTotalPrices(todaySpot, settings),
      tomorrow: calculateTotalPrices(tomorrowSpot, settings),
      tomorrowAvailable: tomorrowSpot.length > 0,
      unit: "c/kWh",
      resolutionMinutes: 15,
    });
  });
};
