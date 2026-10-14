// The HTTP golden cases. Each case runs in a fresh process (empty rate-limit
// stores) against a fresh copy of the seeded database. See the README next to
// the generated fixtures for the case schema and the placeholder syntax.
import { createHmac } from "node:crypto";
import { AUTH_SECRET, userByKey } from "./seed.js";

export { AUTH_SECRET };

export interface RequestDef {
  readonly method: string;
  readonly path: string;
  readonly headers?: Readonly<Record<string, string>>;
  readonly body?: string;
}

export interface VolatileDef {
  readonly pointer: string;
  readonly pattern: string;
}

export interface StepDef {
  readonly request: RequestDef;
  readonly clock?: string;
  readonly repeat?: number;
  readonly volatile?: readonly VolatileDef[];
  readonly special?: "close-db";
  readonly note?: string;
}

export interface CaseDef {
  readonly name: string;
  readonly group: string;
  readonly clock: string;
  readonly profile?: "test" | "production";
  readonly setupSql?: string;
  readonly note?: string;
  readonly steps: readonly StepDef[];
}

// --- Clocks -----------------------------------------------------------------

/** Saturday 2026-10-24, 12:07:30 in Helsinki, before the 12:00 UTC auction. */
export const T_AM = "2026-10-24T09:07:30.000Z";
/** The same day after publication, exactly on a quarter boundary. */
export const T_PM = "2026-10-24T13:00:00.000Z";
/** 03:30 in Helsinki during the repeated hour of the autumn DST change. */
export const T_DST_OCT = "2026-10-25T01:30:00.000Z";
/** Sunday 2026-03-29, the 23-hour spring DST day. */
export const T_DST_MAR = "2026-03-29T10:00:00.000Z";
/** The evening before the spring DST day. */
export const T_MAR_EVE = "2026-03-28T20:00:00.000Z";
/** A day with no stored prices in any area. */
export const T_NODATA = "2026-11-15T12:00:00.000Z";

const plus = (iso: string, seconds: number): string =>
  new Date(Date.parse(iso) + seconds * 1000).toISOString();

// --- Patterns ---------------------------------------------------------------

const P_ID = "^[A-Za-z0-9]{32}$";
const P_API_KEY = "^sp_[0-9a-f]{64}$";

// --- Request helpers ----------------------------------------------------------

const bearer = (userKey: string): Record<string, string> => ({
  authorization: `Bearer ${userByKey(userKey).apiKey}`,
});

const get = (
  path: string,
  headers?: Record<string, string>,
  extra?: Partial<StepDef>,
): StepDef => ({
  request: headers ? { method: "GET", path, headers } : { method: "GET", path },
  ...extra,
});

const send = (
  method: string,
  path: string,
  headers: Record<string, string> | undefined,
  body: string | undefined,
  extra?: Partial<StepDef>,
): StepDef => ({
  request: {
    method,
    path,
    ...(headers ? { headers } : {}),
    ...(body !== undefined ? { body } : {}),
  },
  ...extra,
});

const JSON_CT = { "content-type": "application/json" };

const login = (
  body: unknown,
  headers: Record<string, string> = JSON_CT,
  extra?: Partial<StepDef>,
): StepDef =>
  send(
    "POST",
    "/api/session/login-or-signup",
    headers,
    typeof body === "string" ? body : JSON.stringify(body),
    extra,
  );

/** Better Auth signed cookie value: encodeURIComponent(token "." base64(HMAC-SHA256)). */
export const signedCookieValue = (
  token: string,
  secret = AUTH_SECRET,
): string =>
  encodeURIComponent(
    `${token}.${createHmac("sha256", secret).update(token).digest("base64")}`,
  );

const session = (
  token: string,
  name = "better-auth.session_token",
): Record<string, string> => ({
  cookie: `${name}=${signedCookieValue(token)}`,
});

const ALICE_SESSION = session("goldenTokenAliceValid00000000001");
const GRACE_SESSION = session("goldenTokenGraceValid00000000003");

// --- Cases -------------------------------------------------------------------

const PRICE_ROUTES = ["now", "today", "tomorrow", "all"] as const;

const basicCases: CaseDef[] = [
  {
    name: "health-ok",
    group: "basic",
    clock: T_AM,
    steps: [
      get("/health"),
      send("HEAD", "/health", undefined, undefined),
      get("/health", { "x-real-ip": "203.0.113.9" }),
    ],
  },
  {
    name: "health-db-unavailable",
    group: "basic",
    clock: T_AM,
    note: "The pool is closed before the request; the Rust harness must point the service at an unreachable database.",
    steps: [get("/health", undefined, { special: "close-db" })],
  },
  {
    name: "root-html",
    group: "basic",
    clock: T_AM,
    steps: [get("/"), get("/?utm=1"), send("HEAD", "/", undefined, undefined)],
  },
  {
    name: "not-found",
    group: "basic",
    clock: T_AM,
    steps: [
      get("/nope"),
      get("/health/"),
      get("/api"),
      get("/api/v1"),
      get("/api/v1/nope"),
      get("/api/v1/price"),
      get("/api/v1/price/"),
      get("/api/v1/price/unknown"),
      get("/api/v1/price/unknown", bearer("alice")),
      get("/api/v1/price/NOW", bearer("alice")),
      get("/api/v1/me/unknown"),
      get("/api/v1/me/unknown", ALICE_SESSION),
      get("/api/keys/unknown"),
      get("/api/keys/unknown", ALICE_SESSION),
      get("/api/public/spot/"),
      get("/index.html"),
      get("/api/v1/openapi.json/"),
    ],
  },
  {
    name: "wrong-method",
    group: "basic",
    clock: T_AM,
    steps: [
      send("POST", "/health", undefined, undefined),
      send("DELETE", "/", undefined, undefined),
      send("PUT", "/api/public/spot", undefined, undefined),
      send("POST", "/api/v1/price/now", undefined, undefined),
      send("POST", "/api/v1/price/now", bearer("alice"), undefined),
      send("DELETE", "/api/v1/price/today", bearer("alice"), undefined),
      send("POST", "/api/v1/me/settings", undefined, undefined),
      send("POST", "/api/v1/me/settings", ALICE_SESSION, "{}"),
      send("DELETE", "/api/v1/me/settings", ALICE_SESSION, undefined),
      get("/api/session/login-or-signup"),
      get("/api/session/sign-out"),
      send("POST", "/api/session", undefined, undefined),
      send("POST", "/api/keys", undefined, undefined),
      send("POST", "/api/keys", ALICE_SESSION, undefined),
      get("/api/keys/regenerate", ALICE_SESSION),
      send("POST", "/api/v1/openapi.json", undefined, undefined),
      send("OPTIONS", "/api/v1/price/now", undefined, undefined),
    ],
  },
  {
    name: "openapi-and-docs",
    group: "basic",
    clock: T_AM,
    steps: [get("/api/v1/openapi.json"), get("/api/docs"), get("/api/docs/")],
  },
];

const bearerErrorCases: CaseDef[] = [
  {
    name: "bearer-header-errors",
    group: "bearer-auth",
    clock: T_AM,
    steps: [
      get("/api/v1/price/now"),
      get("/api/v1/price/now", { authorization: "" }),
      get("/api/v1/price/now", { authorization: "Bearer" }),
      get("/api/v1/price/now", { authorization: "Bearer " }),
      get("/api/v1/price/now", {
        authorization: `Basic ${userByKey("alice").apiKey}`,
      }),
      get("/api/v1/price/now", {
        authorization: `Token ${userByKey("alice").apiKey}`,
      }),
      get("/api/v1/price/now", {
        authorization: `${userByKey("alice").apiKey}`,
      }),
      get("/api/v1/price/now", {
        authorization: `Bearer ${userByKey("alice").apiKey} extra`,
      }),
      get("/api/v1/price/now", {
        authorization:
          "Bearer sp_0000000000000000000000000000000000000000000000000000000000000000",
      }),
      get("/api/v1/price/now", { authorization: "Bearer not-a-key" }),
      get("/api/v1/price/now", {
        authorization: `Bearer ${userByKey("alice").apiKey.toUpperCase()}`,
      }),
      get("/api/v1/price/today", { "x-api-key": userByKey("alice").apiKey }),
      get(`/api/v1/price/now?apiKey=${userByKey("alice").apiKey}`),
    ],
  },
  {
    name: "bearer-header-accepted-variants",
    group: "bearer-auth",
    clock: T_AM,
    steps: [
      get("/api/v1/price/now", {
        authorization: `bearer ${userByKey("alice").apiKey}`,
      }),
      get("/api/v1/price/now", {
        authorization: `BEARER ${userByKey("alice").apiKey}`,
      }),
      get("/api/v1/price/now", {
        authorization: `Bearer   ${userByKey("alice").apiKey}`,
      }),
      get("/api/v1/price/now", {
        authorization: `Bearer\t${userByKey("alice").apiKey}`,
      }),
      get("/api/v1/price/now", {
        authorization: ` Bearer ${userByKey("alice").apiKey} `,
      }),
    ],
  },
  {
    name: "bearer-every-route-unauthenticated",
    group: "bearer-auth",
    clock: T_AM,
    steps: [
      ...[
        "now",
        "today",
        "tomorrow",
        "all",
        "cheapest?duration=60",
        "history?from=2026-10-20&to=2026-10-21",
        "forecast",
      ].map((r) => get(`/api/v1/price/${r}`)),
      ...[
        "now",
        "today",
        "tomorrow",
        "all",
        "cheapest?duration=60",
        "history?from=2026-10-20&to=2026-10-21",
        "forecast",
      ].map((r) =>
        get(`/api/v1/price/${r}`, { authorization: "Bearer wrong" }),
      ),
    ],
  },
  {
    name: "user-without-settings",
    group: "bearer-auth",
    clock: T_AM,
    steps: [
      ...PRICE_ROUTES.map((r) => get(`/api/v1/price/${r}`, bearer("grace"))),
      get("/api/v1/price/cheapest?duration=60", bearer("grace")),
      get("/api/v1/price/cheapest", bearer("grace")),
      get(
        "/api/v1/price/history?from=2026-10-20&to=2026-10-21",
        bearer("grace"),
      ),
      get("/api/v1/price/history", bearer("grace")),
      get("/api/v1/price/forecast", bearer("grace")),
      get("/api/v1/price/forecast?area=SE3", bearer("grace")),
      get("/api/v1/price/forecast?area=XX", bearer("grace")),
    ],
  },
];

/** The four list routes for one user at one clock. */
const priceSet = (
  name: string,
  userKey: string,
  clock: string,
  note?: string,
): CaseDef => ({
  name,
  group: "price",
  clock,
  ...(note ? { note } : {}),
  steps: PRICE_ROUTES.map((r) => get(`/api/v1/price/${r}`, bearer(userKey))),
});

const nowAt = (
  name: string,
  userKey: string,
  clock: string,
  note: string,
): CaseDef => ({
  name,
  group: "price-now",
  clock,
  note,
  steps: [get("/api/v1/price/now", bearer(userKey))],
});

const priceCases: CaseDef[] = [
  priceSet(
    "price-alice-fi-helsinki-before-publication",
    "alice",
    T_AM,
    "Tomorrow is the 25-hour day and already stored.",
  ),
  priceSet("price-alice-fi-helsinki-after-publication", "alice", T_PM),
  priceSet(
    "price-bob-se3-tomorrow-unpublished",
    "bob",
    T_AM,
    "SE3 has no rows for local 2026-10-25: tomorrow is available:false with expectedAt.",
  ),
  priceSet("price-bob-se3-after-publication-still-missing", "bob", T_PM),
  priceSet(
    "price-carol-ger-berlin",
    "carol",
    T_AM,
    "Night window 5->5 (start == end).",
  ),
  priceSet(
    "price-dave-ee-partial-tomorrow",
    "dave",
    T_AM,
    "EE publishes only 40 quarters of local 2026-10-25.",
  ),
  priceSet(
    "price-eve-fi-stockholm-zero-terms",
    "eve",
    T_AM,
    "FI prices in a Stockholm day; all contract terms zero; negative spot.",
  ),
  priceSet(
    "price-frank-no1-no-data",
    "frank",
    T_AM,
    "NO1 has no stored prices.",
  ),
  priceSet(
    "price-alice-dst-autumn-today",
    "alice",
    T_DST_OCT,
    "Today is the 25-hour day; tomorrow is not stored.",
  ),
  priceSet(
    "price-alice-dst-spring-today",
    "alice",
    T_DST_MAR,
    "Today is the 23-hour day.",
  ),
  priceSet("price-bob-dst-spring-today", "bob", T_DST_MAR),
  priceSet("price-carol-dst-spring-today", "carol", T_DST_MAR),
  priceSet(
    "price-alice-dst-spring-tomorrow",
    "alice",
    T_MAR_EVE,
    "Tomorrow is the 23-hour day.",
  ),
  priceSet("price-eve-dst-autumn-stockholm", "eve", T_DST_OCT),
  priceSet("price-alice-no-data-anywhere", "alice", T_NODATA),
  nowAt(
    "now-alice-last-ms-of-local-day",
    "alice",
    "2026-10-23T20:59:59.999Z",
    "23:59:59.999 in Helsinki.",
  ),
  nowAt(
    "now-alice-first-ms-of-local-day",
    "alice",
    "2026-10-23T21:00:00.000Z",
    "00:00 in Helsinki.",
  ),
  nowAt(
    "now-alice-negative-500",
    "alice",
    "2026-10-23T23:15:00.000Z",
    "Spot -500 EUR/MWh; total floored.",
  ),
  nowAt(
    "now-alice-negative-zero",
    "alice",
    "2026-10-23T22:45:30.000Z",
    "Spot stored as -0.",
  ),
  nowAt(
    "now-alice-zero",
    "alice",
    "2026-10-23T22:30:00.000Z",
    "Spot exactly 0.",
  ),
  nowAt(
    "now-alice-gap",
    "alice",
    "2026-10-24T15:20:00.000Z",
    "Inside the four-quarter gap: 404.",
  ),
  nowAt(
    "now-alice-spike",
    "alice",
    "2026-10-24T16:05:00.000Z",
    "999.99 EUR/MWh.",
  ),
  nowAt(
    "now-alice-night-start",
    "alice",
    "2026-10-24T19:00:00.000Z",
    "22:00 Helsinki: night starts.",
  ),
  nowAt(
    "now-alice-night-last-quarter",
    "alice",
    "2026-10-24T03:59:59.000Z",
    "06:59 Helsinki: still night.",
  ),
  nowAt(
    "now-alice-day-start",
    "alice",
    "2026-10-24T04:00:00.000Z",
    "07:00 Helsinki: day rate.",
  ),
  nowAt(
    "now-dave-night-23-to-0",
    "dave",
    "2026-10-24T20:00:00.000Z",
    "23:00 Helsinki, window 23->0.",
  ),
  nowAt(
    "now-dave-after-night-23-to-0",
    "dave",
    "2026-10-24T21:00:00.000Z",
    "00:00 Helsinki, window 23->0.",
  ),
  nowAt(
    "now-carol-start-equals-end",
    "carol",
    "2026-10-24T03:00:00.000Z",
    "05:00 Berlin, window 5->5.",
  ),
  nowAt(
    "now-carol-start-equals-end-other-hour",
    "carol",
    "2026-10-24T10:00:00.000Z",
    "12:00 Berlin, window 5->5.",
  ),
  nowAt(
    "now-bob-night-0-to-6",
    "bob",
    "2026-10-23T22:00:00.000Z",
    "00:00 Stockholm, window 0->6.",
  ),
  nowAt(
    "now-bob-day-after-6",
    "bob",
    "2026-10-24T04:00:00.000Z",
    "06:00 Stockholm, window 0->6.",
  ),
  nowAt(
    "now-alice-repeated-hour-first",
    "alice",
    "2026-10-25T00:30:00.000Z",
    "03:30+03:00 Helsinki.",
  ),
  nowAt(
    "now-alice-repeated-hour-second",
    "alice",
    T_DST_OCT,
    "03:30+02:00 Helsinki.",
  ),
  nowAt(
    "now-alice-spring-gap-hour",
    "alice",
    "2026-03-29T01:00:00.000Z",
    "04:00+03:00 Helsinki, just after the skipped hour.",
  ),
  nowAt("now-frank-no-data", "frank", T_AM, "404 No current price available."),
  nowAt(
    "now-alice-no-data",
    "alice",
    T_NODATA,
    "404 No current price available.",
  ),
];

const cheap = (q: string, userKey = "alice"): StepDef =>
  get(`/api/v1/price/cheapest${q}`, bearer(userKey));

const cheapestCases: CaseDef[] = [
  {
    name: "cheapest-success",
    group: "cheapest",
    clock: T_AM,
    steps: [
      cheap("?duration=60"),
      cheap("?duration=15"),
      cheap("?duration=1"),
      cheap("?duration=7"),
      cheap("?duration=100"),
      cheap(
        "?duration=180&startTime=2026-10-24T14:00:00Z&endTime=2026-10-24T20:00:00Z",
      ),
      cheap("?duration=60&maxPrice=12"),
      cheap("?duration=60&maxPrice=%2012%20"),
      cheap("?duration=60&maxPrice=1e1"),
      cheap("?duration=60&maxPrice=0x10"),
      cheap("?duration=1440"),
      cheap("?duration=60&foo=bar"),
      cheap("?duration=60&duration=120"),
      cheap("?duration=060"),
      cheap("?duration=%2060"),
      cheap("?duration=60.0"),
      cheap("?duration=6e1"),
    ],
  },
  {
    name: "cheapest-time-bounds",
    group: "cheapest",
    clock: T_AM,
    steps: [
      cheap("?duration=60&startTime=2026-10-25T05:00:00+03:00"),
      cheap("?duration=60&startTime=2026-10-25T05:00:00%2B03:00"),
      cheap("?duration=60&startTime=2026-10-25T05:00:00%2003:00"),
      cheap("?duration=60&startTime=2026-10-25T02:00:00Z"),
      cheap("?duration=60&startTime=2026-10-25T02:00:00.000Z"),
      cheap("?duration=60&endTime=2026-10-24T12:00:00Z"),
      cheap("?duration=60&startTime=2026-10-24T00:00:00Z"),
      cheap(
        "?duration=60&startTime=2026-10-24T20:00:00Z&endTime=2026-10-24T19:00:00Z",
      ),
      cheap("?duration=60&startTime=2026-12-01T00:00:00Z"),
      cheap(
        "?duration=120&startTime=2026-10-24T14:30:00Z&endTime=2026-10-24T16:30:00Z",
      ),
      cheap("?duration=60&startTime=2026-10-24T14:00:00.123456789+03:00"),
      cheap("?duration=60&startTime=2026-10-24T14:00+03:00"),
      cheap("?duration=60&startTime=2026-10-24T14:00:00+0300"),
    ],
  },
  {
    name: "cheapest-validation-errors",
    group: "cheapest",
    clock: T_AM,
    steps: [
      cheap(""),
      cheap("?duration="),
      cheap("?duration=0"),
      cheap("?duration=1441"),
      cheap("?duration=1.5"),
      cheap("?duration=abc"),
      cheap("?duration=-15"),
      cheap("?duration=Infinity"),
      cheap("?duration=60&startTime=yesterday"),
      cheap("?duration=60&startTime=2026-10-24"),
      cheap("?duration=60&startTime=2026-10-24T14:00:00"),
      cheap("?duration=60&startTime="),
      cheap("?duration=60&endTime=2026-13-01T00:00:00Z"),
      cheap("?duration=60&endTime=not-a-time"),
      cheap("?duration=60&maxPrice="),
      cheap("?duration=60&maxPrice=%20"),
      cheap("?duration=60&maxPrice=abc"),
      cheap("?duration=60&maxPrice=Infinity"),
      cheap("?duration=60&maxPrice=NaN"),
      cheap("?duration=abc&startTime=bad&endTime=bad&maxPrice=x"),
    ],
  },
  {
    name: "cheapest-no-window",
    group: "cheapest",
    clock: T_AM,
    steps: [
      cheap("?duration=60&maxPrice=0"),
      cheap("?duration=60&maxPrice=-1"),
      cheap("?duration=60&maxPrice=0.5"),
      cheap("?duration=60&maxPrice=1e-7"),
      cheap("?duration=1440", "bob"),
      cheap("?duration=60", "frank"),
      cheap("?duration=1440&startTime=2026-10-25T00:00:00Z"),
      cheap("?duration=60&maxPrice=10", "bob"),
    ],
  },
  {
    name: "cheapest-other-users",
    group: "cheapest",
    clock: T_PM,
    steps: [
      cheap("?duration=120", "bob"),
      cheap("?duration=120", "carol"),
      cheap("?duration=120", "dave"),
      cheap("?duration=120", "eve"),
      cheap("?duration=45&maxPrice=0", "eve"),
    ],
  },
  {
    name: "cheapest-dst",
    group: "cheapest",
    clock: T_DST_OCT,
    steps: [
      cheap("?duration=120"),
      cheap("?duration=120", "carol"),
      cheap("?duration=240&endTime=2026-10-25T05:00:00%2B02:00"),
    ],
  },
  {
    name: "cheapest-dst-spring",
    group: "cheapest",
    clock: T_MAR_EVE,
    steps: [
      cheap("?duration=180"),
      cheap("?duration=180", "bob"),
      cheap("?duration=180", "carol"),
    ],
  },
];

const hist = (q: string, userKey = "alice"): StepDef =>
  get(`/api/v1/price/history${q}`, bearer(userKey));

const historyCases: CaseDef[] = [
  {
    name: "history-success",
    group: "history",
    clock: T_AM,
    steps: [
      hist("?from=2026-10-23&to=2026-10-24"),
      hist("?from=2026-10-25&to=2026-10-25"),
      hist("?from=2026-03-28&to=2026-03-30"),
      hist("?from=2026-01-01&to=2026-01-31"),
      hist("?from=2026-10-26&to=2026-10-27"),
      hist("?from=2026-10-22&to=2026-10-23", "bob"),
      hist("?from=2026-10-24&to=2026-10-25", "dave"),
      hist("?from=2026-10-24&to=2026-10-24", "frank"),
      hist("?from=2026-03-29&to=2026-03-29", "carol"),
      hist("?to=2026-10-24&from=2026-10-24&extra=1"),
    ],
  },
  {
    name: "history-validation-errors",
    group: "history",
    clock: T_AM,
    steps: [
      hist(""),
      hist("?from=2026-10-20"),
      hist("?to=2026-10-20"),
      hist("?from=&to="),
      hist("?from=2026-10-24&to=2026-10-23"),
      hist("?from=2026-01-01&to=2026-02-01"),
      hist("?from=2025-01-01&to=2026-12-31"),
      hist("?from=2026-02-30&to=2026-03-01"),
      hist("?from=2026-02-29&to=2026-03-01"),
      hist("?from=2028-02-29&to=2028-03-01"),
      hist("?from=2026-13-01&to=2026-13-02"),
      hist("?from=20261001&to=20261002"),
      hist("?from=2026-10-1&to=2026-10-2"),
      hist("?from=2026-10-01T00:00:00Z&to=2026-10-02"),
      hist("?from=abc&to=def"),
      hist("?from=2026-10-25&to=abc"),
    ],
  },
];

const fc = (q: string, userKey = "alice"): StepDef =>
  get(`/api/v1/price/forecast${q}`, bearer(userKey));

const forecastCases: CaseDef[] = [
  {
    name: "forecast-success",
    group: "forecast",
    clock: T_AM,
    steps: [fc(""), fc("?area=FI")],
  },
  {
    name: "forecast-success-stockholm-user",
    group: "forecast",
    clock: T_PM,
    steps: [fc("", "bob"), fc("", "carol")],
  },
  {
    name: "forecast-non-fi-and-errors",
    group: "forecast",
    clock: T_AM,
    steps: [
      fc("?area=SE3"),
      fc("?area=GER"),
      fc("?area=XX"),
      fc("?area=fi"),
      fc("?area="),
      fc("?area=FI&area=SE3"),
    ],
  },
  {
    name: "forecast-no-fingrid-data",
    group: "forecast",
    clock: T_AM,
    setupSql: "DELETE FROM fingrid_actuals; DELETE FROM fingrid_forecasts;",
    steps: [fc("")],
  },
  {
    name: "forecast-one-day-history",
    note: "About one day of FI history still fits the model.",
    group: "forecast",
    clock: T_AM,
    setupSql:
      "DELETE FROM prices WHERE area = 'FI' AND delivery_start < '2026-10-25T00:00:00Z';",
    steps: [fc("")],
  },
  {
    name: "forecast-partial-fingrid-outage",
    note: "Vintages stop at 2026-10-27; the model fills the tail without zero-seeding.",
    group: "forecast",
    clock: T_AM,
    setupSql:
      "DELETE FROM fingrid_forecasts WHERE start_time >= '2026-10-27T00:00:00Z';",
    steps: [fc("")],
  },
  {
    name: "forecast-actuals-only",
    note: "No forecast vintages; actuals only.",
    group: "forecast",
    clock: T_AM,
    setupSql: "DELETE FROM fingrid_forecasts;",
    steps: [fc("")],
  },
  {
    name: "forecast-thin-history-fallback",
    note: "20 FI quarters (< MIN_FIT_SAMPLES = 24): default estimate.",
    group: "forecast",
    clock: T_AM,
    setupSql:
      "DELETE FROM prices WHERE area = 'FI' AND delivery_start < '2026-10-25T17:00:00Z';",
    steps: [fc("")],
  },
  {
    name: "forecast-sparse-fingrid",
    note: "Only about one day of actuals at the start of the history window.",
    group: "forecast",
    clock: T_AM,
    setupSql:
      "DELETE FROM fingrid_forecasts; DELETE FROM fingrid_actuals WHERE start_time >= '2026-09-27T00:00:00Z';",
    steps: [fc("")],
  },
  {
    name: "forecast-no-fi-prices",
    note: "No FI price at all: the series starts at the clock instant, not after the last price.",
    group: "forecast",
    clock: T_AM,
    setupSql: "DELETE FROM prices WHERE area = 'FI';",
    steps: [fc("")],
  },
];

const spot = (q: string, headers?: Record<string, string>): StepDef =>
  get(`/api/public/spot${q}`, headers);

const publicCases: CaseDef[] = [
  {
    name: "public-spot",
    group: "public",
    clock: T_AM,
    steps: [
      spot(""),
      spot("?area=SE3"),
      spot("?area=EE"),
      spot("?area=NO1"),
      spot("?area=GER"),
      spot("?area=XX"),
      spot("?area=fi"),
      spot("?area="),
      spot("", bearer("bob")),
    ],
  },
  {
    name: "public-spot-dst",
    group: "public",
    clock: T_DST_OCT,
    steps: [spot(""), spot("?area=SE3")],
  },
  {
    name: "public-spot-dst-spring",
    group: "public",
    clock: T_DST_MAR,
    steps: [spot("?area=GER")],
  },
];

// --- Session flows -------------------------------------------------------------

const SIGNUP_VOLATILE: VolatileDef[] = [
  { pointer: "/token", pattern: P_ID },
  { pointer: "/user/id", pattern: P_ID },
];
const SESSION_VOLATILE: VolatileDef[] = [
  { pointer: "/session/session/id", pattern: P_ID },
  { pointer: "/session/session/token", pattern: P_ID },
  { pointer: "/session/session/userId", pattern: P_ID },
  { pointer: "/session/user/id", pattern: P_ID },
];
const KEY_VOLATILE: VolatileDef[] = [
  { pointer: "/apiKey", pattern: P_API_KEY },
];

const cookieOf = (
  step: number,
  name = "better-auth.session_token",
): Record<string, string> => ({
  cookie: `{{s${String(step)}.cookie:${name}}}`,
});
const keyOf = (step: number): Record<string, string> => ({
  authorization: `Bearer {{s${String(step)}.json:/apiKey}}`,
});

const sessionCases: CaseDef[] = [
  {
    name: "session-signup-full-flow",
    group: "session",
    clock: T_AM,
    steps: [
      login({ username: "newuser", password: "new-user-pass-1" }, JSON_CT, {
        volatile: SIGNUP_VOLATILE,
      }),
      get("/api/session", cookieOf(0), { volatile: SESSION_VOLATILE }),
      get("/api/keys", cookieOf(0), { volatile: KEY_VOLATILE }),
      get("/api/keys", cookieOf(0), { volatile: KEY_VOLATILE }),
      get("/api/v1/price/now", keyOf(2)),
      send("POST", "/api/keys/regenerate", cookieOf(0), undefined, {
        volatile: KEY_VOLATILE,
      }),
      get("/api/v1/price/now", keyOf(2)),
      get("/api/v1/price/now", keyOf(5)),
      get("/api/v1/me/settings", cookieOf(0), {
        volatile: [{ pointer: "/userId", pattern: P_ID }],
      }),
      send(
        "PUT",
        "/api/v1/me/settings",
        { ...cookieOf(0), ...JSON_CT },
        JSON.stringify({ marginCentsKwh: 1.5, timezone: "Europe/Stockholm" }),
        { volatile: [{ pointer: "/userId", pattern: P_ID }] },
      ),
      get("/api/v1/me/settings", cookieOf(0), {
        volatile: [{ pointer: "/userId", pattern: P_ID }],
      }),
      get("/api/v1/me/chart", cookieOf(0)),
      get("/api/v1/price/today", keyOf(5)),
      send("POST", "/api/session/sign-out", cookieOf(0), undefined),
      get("/api/session", cookieOf(0)),
      get("/api/keys", cookieOf(0)),
      get("/api/v1/me/settings", cookieOf(0)),
      get("/api/v1/price/now", keyOf(5)),
    ],
  },
  {
    name: "session-login-existing-user",
    group: "session",
    clock: T_AM,
    steps: [
      login({ username: "alice", password: "alice-password-1" }, JSON_CT, {
        volatile: [{ pointer: "/token", pattern: P_ID }],
      }),
      get("/api/session", cookieOf(0), {
        volatile: [
          { pointer: "/session/session/id", pattern: P_ID },
          { pointer: "/session/session/token", pattern: P_ID },
        ],
      }),
      get("/api/keys", cookieOf(0)),
      get("/api/v1/me/settings", cookieOf(0)),
      get("/api/v1/me/chart", cookieOf(0)),
      login({ username: "  ALICE ", password: "alice-password-1" }, JSON_CT, {
        volatile: [{ pointer: "/token", pattern: P_ID }],
      }),
      login({ username: "alice", password: "wrong-password" }),
      login({ username: "alice", password: "short" }),
      login({ username: "alice", password: "x".repeat(129) }),
      send("POST", "/api/session/sign-out", cookieOf(0), undefined),
      send("POST", "/api/session/sign-out", undefined, undefined),
    ],
  },
  {
    name: "session-login-payload-errors",
    group: "session",
    clock: T_AM,
    steps: [
      login("{"),
      login("null"),
      login("[]"),
      login('"text"'),
      login("{}"),
      login({ username: "alice" }),
      login({ username: "alice", password: "" }),
      login({ password: "alice-password-1" }),
      login({ username: "ab", password: "long-enough-1" }),
      login({ username: "a".repeat(33), password: "long-enough-1" }),
    ],
  },
  {
    name: "session-login-payload-errors-2",
    group: "session",
    clock: T_AM,
    steps: [
      login({ username: "has.dot", password: "long-enough-1" }),
      login({ username: "has space", password: "long-enough-1" }),
      login({ username: "äiti", password: "long-enough-1" }),
      login({ username: "alice", password: 12345678 }),
      login({ username: 12345, password: "long-enough-1" }),
      login(
        { username: "alice", password: "alice-password-1" },
        { "content-type": "text/plain" },
        { volatile: [{ pointer: "/token", pattern: P_ID }] },
      ),
      login(
        { username: "alice", password: "alice-password-1" },
        {},
        { volatile: [{ pointer: "/token", pattern: P_ID }] },
      ),
      login("username=alice&password=alice-password-1", {
        "content-type": "application/x-www-form-urlencoded",
      }),
      login(
        { username: "alice", password: "alice-password-1", extra: true },
        JSON_CT,
        { volatile: [{ pointer: "/token", pattern: P_ID }] },
      ),
    ],
  },
  {
    name: "session-signup-password-rules",
    group: "session",
    clock: T_AM,
    steps: [
      login({ username: "shortpw", password: "1234567" }),
      login({ username: "longpw", password: "p".repeat(129) }),
      login({ username: "minpw", password: "12345678" }, JSON_CT, {
        volatile: SIGNUP_VOLATILE,
      }),
      login({ username: "maxpw", password: "p".repeat(128) }, JSON_CT, {
        volatile: SIGNUP_VOLATILE,
      }),
      login({ username: "nfkcpw", password: "ｐａｓｓｗｏｒｄ１２" }, JSON_CT, {
        volatile: SIGNUP_VOLATILE,
      }),
      login({ username: "nfkcpw", password: "password12" }, JSON_CT, {
        volatile: [
          { pointer: "/token", pattern: P_ID },
          { pointer: "/user/id", pattern: P_ID },
        ],
      }),
      login({ username: "emoji7", password: "🔑🔑🔑🔑" }, JSON_CT, {
        volatile: SIGNUP_VOLATILE,
      }),
      login({ username: "Mixed-Case_1", password: "mixed-case-pw" }, JSON_CT, {
        volatile: SIGNUP_VOLATILE,
      }),
    ],
  },
  {
    name: "session-signup-existing-email",
    group: "session",
    clock: T_AM,
    setupSql: `INSERT INTO "user" ("id", "name", "email", "emailVerified", "image", "createdAt", "updatedAt") VALUES ('goldenUserOrphan0000000000000008', 'zed', 'zed@local.spot', false, NULL, '2026-09-02T00:00:00Z', '2026-09-02T00:00:00Z');`,
    note: "A user row without a usernames row makes login-or-signup try a sign-up for an existing email.",
    steps: [login({ username: "zed", password: "zed-password-1" })],
  },
  {
    name: "session-registration-closed",
    group: "session",
    clock: T_AM,
    setupSql: `INSERT INTO "user" ("id", "name", "email", "emailVerified", "createdAt", "updatedAt") SELECT 'filler' || g, 'filler' || g, 'filler' || g || '@local.spot', false, '2026-09-03T00:00:00Z', '2026-09-03T00:00:00Z' FROM generate_series(1, 99993) AS g;`,
    note: "The seed has 7 users; 99 993 filler rows reach MAX_USERS = 100 000.",
    steps: [
      login({ username: "latecomer", password: "late-password-1" }),
      login({ username: "alice", password: "alice-password-1" }, JSON_CT, {
        volatile: [{ pointer: "/token", pattern: P_ID }],
      }),
      login({ username: "x", password: "late-password-1" }),
    ],
  },
  {
    name: "session-registration-one-below-cap",
    group: "session",
    clock: T_AM,
    setupSql: `INSERT INTO "user" ("id", "name", "email", "emailVerified", "createdAt", "updatedAt") SELECT 'filler' || g, 'filler' || g, 'filler' || g || '@local.spot', false, '2026-09-03T00:00:00Z', '2026-09-03T00:00:00Z' FROM generate_series(1, 99992) AS g;`,
    steps: [
      login({ username: "lastone", password: "last-password-1" }, JSON_CT, {
        volatile: SIGNUP_VOLATILE,
      }),
      login({ username: "toolate", password: "late-password-1" }),
    ],
  },
  {
    name: "session-csrf-headers",
    group: "session",
    clock: T_AM,
    steps: [
      login(
        { username: "alice", password: "alice-password-1" },
        { ...JSON_CT, origin: "https://evil.example" },
        { volatile: [{ pointer: "/token", pattern: P_ID }] },
      ),
      login(
        { username: "alice", password: "alice-password-1" },
        {
          ...JSON_CT,
          "sec-fetch-site": "cross-site",
          "sec-fetch-mode": "navigate",
          "sec-fetch-dest": "document",
        },
        { volatile: [{ pointer: "/token", pattern: P_ID }] },
      ),
      login(
        { username: "alice", password: "alice-password-1" },
        { ...JSON_CT, ...ALICE_SESSION, origin: "https://evil.example" },
        { volatile: [{ pointer: "/token", pattern: P_ID }] },
      ),
      login(
        { username: "csrfnew", password: "csrf-password-1" },
        {
          ...JSON_CT,
          origin: "https://evil.example",
          "sec-fetch-site": "cross-site",
          "sec-fetch-mode": "navigate",
        },
        { volatile: SIGNUP_VOLATILE },
      ),
    ],
  },
  {
    name: "session-seeded-cookies",
    group: "session",
    clock: T_AM,
    steps: [
      get("/api/session"),
      get("/api/session", ALICE_SESSION),
      get("/api/session", session("goldenTokenAliceExpired000000002")),
      get("/api/session", session("goldenTokenBobOld000000000000004")),
      get("/api/session", session("goldenTokenBobOld000000000000004")),
      get("/api/session", { cookie: "better-auth.session_token=garbage" }),
      get("/api/session", {
        cookie: "better-auth.session_token=goldenTokenAliceValid00000000001",
      }),
      get("/api/session", {
        cookie: `better-auth.session_token=${signedCookieValue("goldenTokenAliceValid00000000001", "wrong-secret-wrong-secret-wrong-secret")}`,
      }),
      get("/api/session", {
        cookie: `__Secure-better-auth.session_token=${signedCookieValue("goldenTokenAliceValid00000000001")}`,
      }),
      get("/api/session", {
        cookie: `other=1; better-auth.session_token=${signedCookieValue("goldenTokenAliceValid00000000001")}; another=2`,
      }),
      get("/api/session", {
        authorization: "Bearer goldenTokenAliceValid00000000001",
      }),
      get("/api/keys"),
      send("POST", "/api/keys/regenerate", undefined, undefined),
      get("/api/keys", session("goldenTokenAliceExpired000000002")),
      get("/api/v1/me/settings"),
      get("/api/v1/me/settings", session("goldenTokenAliceExpired000000002")),
      get("/api/v1/me/chart"),
      get("/api/v1/me/settings", bearer("alice")),
    ],
  },
  {
    name: "session-grace-defaults-created",
    group: "session",
    clock: T_AM,
    note: "grace has no user_settings row; the session routes create the defaults.",
    steps: [
      get("/api/v1/price/now", bearer("grace")),
      get("/api/v1/me/settings", GRACE_SESSION),
      get("/api/v1/price/now", bearer("grace")),
      get("/api/keys", GRACE_SESSION),
      send("POST", "/api/keys/regenerate", GRACE_SESSION, undefined, {
        volatile: KEY_VOLATILE,
      }),
      get("/api/v1/price/now", bearer("grace")),
      get("/api/keys", GRACE_SESSION, { volatile: KEY_VOLATILE }),
    ],
  },
  {
    name: "session-chart",
    group: "session",
    clock: T_AM,
    steps: [
      get("/api/v1/me/chart", ALICE_SESSION),
      get("/api/v1/me/chart", session("goldenTokenBobOld000000000000004")),
      get("/api/v1/me/chart", GRACE_SESSION),
    ],
  },
  {
    name: "session-chart-dst",
    group: "session",
    clock: T_DST_OCT,
    steps: [get("/api/v1/me/chart", ALICE_SESSION)],
  },
];

const put = (
  body: string,
  headers: Record<string, string> = { ...ALICE_SESSION, ...JSON_CT },
): StepDef => send("PUT", "/api/v1/me/settings", headers, body);

const settingsCases: CaseDef[] = [
  {
    name: "settings-put-valid",
    group: "settings",
    clock: T_AM,
    steps: [
      get("/api/v1/me/settings", ALICE_SESSION),
      put(JSON.stringify({ vatPercent: 24 })),
      put(JSON.stringify({})),
      put(JSON.stringify({ foo: 1 })),
      put(
        JSON.stringify({
          marginCentsKwh: 0,
          transferDayCentsKwh: 0,
          transferNightCentsKwh: 0,
          taxCentsKwh: 0,
          vatPercent: 0,
          nightStartHour: 0,
          nightEndHour: 0,
        }),
      ),
      put(
        JSON.stringify({
          vatPercent: 100,
          nightStartHour: 23,
          nightEndHour: 23,
          timezone: "Europe/Berlin",
          area: "GER",
        }),
      ),
      put(
        JSON.stringify({
          marginCentsKwh: 0.1 + 0.2,
          taxCentsKwh: 1e-7,
          transferDayCentsKwh: 12345.678901234,
        }),
      ),
      put(JSON.stringify({ marginCentsKwh: 1, userId: "someone-else" })),
      get("/api/v1/me/settings", ALICE_SESSION),
      get("/api/v1/price/now", bearer("alice")),
      put('{"vatPercent": 25.5, "vatPercent": 10}'),
      put(
        JSON.stringify({
          timezone: "Europe/Helsinki",
          area: "FI",
          nightStartHour: 22,
          nightEndHour: 7,
          vatPercent: 25.5,
        }),
        { ...ALICE_SESSION, "content-type": "application/json; charset=utf-8" },
      ),
      get("/api/v1/me/settings", ALICE_SESSION),
    ],
  },
  {
    name: "settings-put-validation-errors",
    group: "settings",
    clock: T_AM,
    steps: [
      put(JSON.stringify({ marginCentsKwh: -1 })),
      put(JSON.stringify({ transferDayCentsKwh: "1.5" })),
      put(JSON.stringify({ transferNightCentsKwh: -0.01 })),
      put(JSON.stringify({ taxCentsKwh: null })),
      put(JSON.stringify({ vatPercent: 101 })),
      put(JSON.stringify({ vatPercent: -1 })),
      put(JSON.stringify({ nightStartHour: 24 })),
      put(JSON.stringify({ nightStartHour: 1.5 })),
      put(JSON.stringify({ nightEndHour: -1 })),
      put(JSON.stringify({ nightEndHour: "7" })),
      put(JSON.stringify({ timezone: "Mars/Base" })),
      put(JSON.stringify({ timezone: "europe/helsinki" })),
      put(JSON.stringify({ area: "XX" })),
      put(JSON.stringify({ area: "fi" })),
      put(JSON.stringify({ area: 1 })),
      put(JSON.stringify({ marginCentsKwh: true })),
      put(
        JSON.stringify({
          marginCentsKwh: -1,
          vatPercent: 200,
          nightStartHour: 30,
          timezone: "X",
          area: "Y",
        }),
      ),
      put("1e400"),
      put(JSON.stringify({ marginCentsKwh: 1e308 * 10 })),
      get("/api/v1/me/settings", ALICE_SESSION),
    ],
  },
  {
    name: "settings-put-body-errors",
    group: "settings",
    clock: T_AM,
    steps: [
      put("{"),
      put(""),
      put("null"),
      put("[]"),
      put('"text"'),
      put("42"),
      put(JSON.stringify({ vatPercent: 10 }), { ...ALICE_SESSION }),
      put(JSON.stringify({ vatPercent: 11 }), {
        ...ALICE_SESSION,
        "content-type": "text/plain",
      }),
      put("{", { ...ALICE_SESSION, "content-type": "text/plain" }),
      put("vatPercent=12", {
        ...ALICE_SESSION,
        "content-type": "application/x-www-form-urlencoded",
      }),
      put(JSON.stringify({ vatPercent: 13 }), {
        ...ALICE_SESSION,
        "content-type": "application/vnd.api+json",
      }),
      put(JSON.stringify({ vatPercent: 14 }), {
        ...ALICE_SESSION,
        "content-type": "APPLICATION/JSON",
      }),
      put(JSON.stringify({ vatPercent: 15 }), { ...JSON_CT }),
      get("/api/v1/me/settings", ALICE_SESSION),
    ],
  },
];

// --- Rate limits ---------------------------------------------------------------

const RL_PATH = "/api/public/spot?area=NO1";

const rateLimitCases: CaseDef[] = [
  {
    name: "ratelimit-global-unknown-ip",
    group: "ratelimit",
    clock: T_AM,
    steps: [
      get(RL_PATH),
      get(RL_PATH, undefined, { repeat: 119 }),
      get(RL_PATH),
      get(RL_PATH),
      get("/health"),
      get("/"),
      get("/nope"),
      get("/api/v1/price/now", bearer("alice")),
      get(RL_PATH, undefined, { clock: plus(T_AM, 30) }),
      get(RL_PATH, undefined, { clock: plus(T_AM, 59.999) }),
      get(RL_PATH, undefined, { clock: plus(T_AM, 60) }),
      get(RL_PATH, undefined, { clock: plus(T_AM, 61) }),
    ],
  },
  {
    name: "ratelimit-global-x-real-ip",
    group: "ratelimit",
    clock: T_AM,
    steps: [
      get(RL_PATH, { "x-real-ip": "198.51.100.1" }, { repeat: 120 }),
      get(RL_PATH, { "x-real-ip": "198.51.100.1" }),
      get(RL_PATH, { "x-real-ip": "198.51.100.2" }),
      get(RL_PATH, { "x-forwarded-for": "198.51.100.1, 10.0.0.1" }),
      get(RL_PATH, {
        "x-real-ip": "198.51.100.1",
        "x-forwarded-for": "198.51.100.9",
      }),
      get(RL_PATH, {
        "x-real-ip": "198.51.100.9",
        "x-forwarded-for": "198.51.100.1",
      }),
      get(RL_PATH),
      get(RL_PATH, { "x-real-ip": "" }),
      get(RL_PATH, { "x-real-ip": "198.51.100.1 " }),
    ],
  },
  {
    name: "ratelimit-global-x-forwarded-for",
    group: "ratelimit",
    clock: T_AM,
    steps: [
      get(
        RL_PATH,
        { "x-forwarded-for": "192.0.2.7, 10.0.0.1" },
        { repeat: 120 },
      ),
      get(RL_PATH, { "x-forwarded-for": " 192.0.2.7 " }),
      get(RL_PATH, { "x-forwarded-for": "192.0.2.7" }),
      get(RL_PATH, { "x-forwarded-for": "10.0.0.1, 192.0.2.7" }),
      get(RL_PATH, { "x-forwarded-for": "" }),
      get(RL_PATH, { "x-forwarded-for": ",192.0.2.7" }),
    ],
  },
  {
    name: "ratelimit-global-on-bearer-route",
    group: "ratelimit",
    clock: T_AM,
    steps: [
      get("/api/v1/price/now", bearer("alice"), { repeat: 60 }),
      get("/api/v1/price/now", bearer("bob"), { repeat: 60 }),
      get("/api/v1/price/now", bearer("carol")),
      get("/api/v1/price/now"),
      get("/health"),
    ],
  },
  {
    name: "ratelimit-per-user",
    group: "ratelimit",
    clock: T_AM,
    steps: [
      get("/api/v1/price/now", bearer("alice")),
      get("/api/v1/price/now", bearer("alice"), { repeat: 58 }),
      get("/api/v1/price/now", bearer("alice")),
      get("/api/v1/price/now", bearer("alice")),
      get("/api/v1/price/today", bearer("alice")),
      get("/api/v1/price/forecast", bearer("alice")),
      get("/api/v1/price/now", {
        ...bearer("alice"),
        "x-real-ip": "198.51.100.77",
      }),
      get("/api/v1/price/now", bearer("bob")),
      get("/api/v1/price/now", { authorization: "Bearer invalid" }),
      get("/api/v1/price/now"),
      get(RL_PATH),
      get("/api/v1/price/now", bearer("alice"), { clock: plus(T_AM, 60) }),
    ],
  },
  {
    name: "ratelimit-login",
    group: "ratelimit",
    clock: T_AM,
    steps: [
      login({ username: "alice", password: "wrong-password" }),
      login({ username: "alice", password: "wrong-password" }, JSON_CT, {
        repeat: 8,
      }),
      login("{"),
      login({ username: "alice", password: "alice-password-1" }),
      login(
        { username: "alice", password: "alice-password-1" },
        { ...JSON_CT, "x-real-ip": "198.51.100.50" },
        { volatile: [{ pointer: "/token", pattern: P_ID }] },
      ),
      get("/api/session"),
      login({ username: "alice", password: "alice-password-1" }, JSON_CT, {
        clock: plus(T_AM, 899),
      }),
      login({ username: "alice", password: "alice-password-1" }, JSON_CT, {
        clock: plus(T_AM, 900),
        volatile: [{ pointer: "/token", pattern: P_ID }],
      }),
    ],
  },
];

// --- Production profile ------------------------------------------------------------

const PROD_COOKIE = "__Secure-better-auth.session_token";

const productionCases: CaseDef[] = [
  {
    name: "production-cookie-flow",
    group: "production",
    clock: T_AM,
    profile: "production",
    note: "NODE_ENV=production and BETTER_AUTH_URL=https://spot.calmdonut.com, as on Railway.",
    steps: [
      login({ username: "produser", password: "prod-password-1" }, JSON_CT, {
        volatile: SIGNUP_VOLATILE,
      }),
      get("/api/session", cookieOf(0, PROD_COOKIE), {
        volatile: SESSION_VOLATILE,
      }),
      login({ username: "alice", password: "alice-password-1" }, JSON_CT, {
        volatile: [{ pointer: "/token", pattern: P_ID }],
      }),
      get(
        "/api/session",
        session("goldenTokenAliceValid00000000001", PROD_COOKIE),
      ),
      get("/api/session", ALICE_SESSION),
      send(
        "POST",
        "/api/session/sign-out",
        cookieOf(0, PROD_COOKIE),
        undefined,
      ),
      get("/api/session", cookieOf(0, PROD_COOKIE)),
      get("/health"),
      get("/api/v1/price/now", bearer("alice")),
    ],
  },
];

export const CASES: readonly CaseDef[] = [
  ...basicCases,
  ...bearerErrorCases,
  ...priceCases,
  ...cheapestCases,
  ...historyCases,
  ...forecastCases,
  ...publicCases,
  ...sessionCases,
  ...settingsCases,
  ...rateLimitCases,
  ...productionCases,
];
