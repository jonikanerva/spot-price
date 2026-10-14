// Builds the shared seed of the HTTP golden cases: public prices, Fingrid
// rows, and seven complete users. The values come from a fixed LCG, so the
// SQL is the same on every run. The seed calendar is in
// crates/server/tests/golden/http/README.md; keep the two in step.

const QUARTER_MS = 15 * 60 * 1000;
const HOUR_MS = 60 * 60 * 1000;
const DAY_MS = 24 * HOUR_MS;

let lcg = 20261024;
const rand = (): number => {
  lcg = (Math.imul(lcg, 1103515245) + 12345) & 0x7fffffff;
  return lcg / 0x7fffffff;
};

const ms = (iso: string): number => Date.parse(iso);
const r2 = (x: number): number => Math.round(x * 100) / 100;
const r1 = (x: number): number => Math.round(x * 10) / 10;

/** The fraction of a UTC day, 0 at 00:00Z. */
const dayPhase = (t: number): number => (t % DAY_MS) / DAY_MS;

const wind = (t: number): number =>
  r1(
    2600 +
      1700 *
        Math.sin(
          (2 * Math.PI * (t - ms("2026-09-01T00:00:00Z"))) / (5.3 * DAY_MS),
        ) +
      400 * Math.sin(2 * Math.PI * dayPhase(t)) +
      250 * (rand() - 0.5),
  );

const consumption = (t: number): number =>
  r1(
    9000 +
      1300 * Math.sin(2 * Math.PI * (dayPhase(t) - 0.3)) +
      300 * (rand() - 0.5),
  );

/** EUR/MWh from the grid balance, so the forecast fit finds a real slope. */
const priceFrom = (t: number, bias: number): number =>
  r2(bias + 0.012 * (consumption(t) - wind(t)) - 40 + 12 * (rand() - 0.5));

const sqlNum = (v: number | null): string => {
  if (v === null) {
    return "NULL";
  }
  return Object.is(v, -0) ? "-0" : String(v);
};

/** One INSERT for a run of consecutive quarters; a null value is a gap. */
const quarterRun = (
  table: string,
  columns: string,
  extra: string,
  startIso: string,
  values: readonly (number | null)[],
): string =>
  `INSERT INTO ${table} (${columns})\n` +
  `SELECT t, t + interval '15 minutes', v${extra}\n` +
  `FROM unnest('{${values.map(sqlNum).join(",")}}'::double precision[]) WITH ORDINALITY AS u(v, i)\n` +
  `CROSS JOIN LATERAL (SELECT timestamptz '${startIso}' + (i - 1) * interval '15 minutes' AS t) AS s\n` +
  `WHERE v IS NOT NULL;\n`;

const quarters = (fromIso: string, toIso: string): number[] => {
  const out: number[] = [];
  for (let t = ms(fromIso); t < ms(toIso); t += QUARTER_MS) {
    out.push(t);
  }
  return out;
};

interface PriceWindow {
  readonly area: string;
  readonly from: string;
  readonly to: string;
  readonly bias: number;
  /** Overrides by UTC quarter start; null deletes the quarter (a gap). */
  readonly overrides?: Readonly<Record<string, number | null>>;
}

export const PRICE_WINDOWS: readonly PriceWindow[] = [
  // Spring: every DST-affected area covers local 2026-03-27..2026-03-30.
  {
    area: "FI",
    from: "2026-03-26T00:00:00Z",
    to: "2026-03-31T00:00:00Z",
    bias: 55,
  },
  {
    area: "SE3",
    from: "2026-03-26T00:00:00Z",
    to: "2026-03-31T00:00:00Z",
    bias: 35,
  },
  {
    area: "GER",
    from: "2026-03-26T00:00:00Z",
    to: "2026-03-31T00:00:00Z",
    bias: 80,
  },
  {
    area: "EE",
    from: "2026-03-26T00:00:00Z",
    to: "2026-03-31T00:00:00Z",
    bias: 60,
  },
  // Autumn. FI ends at the end of local 2026-10-25 (the 25-hour day), which
  // also anchors the forecast series at 2026-10-25T22:00:00Z.
  {
    area: "FI",
    from: "2026-09-20T00:00:00Z",
    to: "2026-10-25T22:00:00Z",
    bias: 55,
    overrides: {
      // Negative and zero prices on the night of local 2026-10-24.
      "2026-10-23T22:00:00Z": -1.5,
      "2026-10-23T22:15:00Z": -0.01,
      "2026-10-23T22:30:00Z": 0,
      "2026-10-23T22:45:00Z": -0,
      "2026-10-23T23:00:00Z": -12.34,
      "2026-10-23T23:15:00Z": -500,
      "2026-10-23T23:30:00Z": 0,
      "2026-10-23T23:45:00Z": 0.01,
      // A four-quarter gap on the afternoon of local 2026-10-24.
      "2026-10-24T15:00:00Z": null,
      "2026-10-24T15:15:00Z": null,
      "2026-10-24T15:30:00Z": null,
      "2026-10-24T15:45:00Z": null,
      // A cheap block the cheapest-window search must find.
      "2026-10-25T02:00:00Z": 1.11,
      "2026-10-25T02:15:00Z": 1.12,
      "2026-10-25T02:30:00Z": 1.13,
      "2026-10-25T02:45:00Z": 1.14,
      // A price spike.
      "2026-10-24T16:00:00Z": 999.99,
    },
  },
  // SE1 feeds the forecast as a neighbour over the whole history window.
  {
    area: "SE1",
    from: "2026-09-20T00:00:00Z",
    to: "2026-10-25T23:00:00Z",
    bias: 30,
  },
  // SE3 ends at the end of local 2026-10-24: tomorrow is not published.
  {
    area: "SE3",
    from: "2026-10-20T00:00:00Z",
    to: "2026-10-24T22:00:00Z",
    bias: 35,
  },
  // EE publishes only the first 40 quarters of local 2026-10-25.
  {
    area: "EE",
    from: "2026-10-20T00:00:00Z",
    to: "2026-10-25T07:00:00Z",
    bias: 60,
  },
  // GER covers local 2026-10-21..2026-10-25 completely.
  {
    area: "GER",
    from: "2026-10-20T22:00:00Z",
    to: "2026-10-25T23:00:00Z",
    bias: 80,
  },
];

export interface SeedUser {
  readonly key: string;
  readonly id: string;
  readonly username: string;
  readonly password: string;
  readonly apiKey: string;
  readonly settings: {
    readonly margin: number;
    readonly transferDay: number;
    readonly transferNight: number;
    readonly tax: number;
    readonly vat: number;
    readonly nightStart: number;
    readonly nightEnd: number;
    readonly timezone: string;
    readonly area: string;
  } | null;
}

const key = (pair: string): string => `sp_${pair.repeat(32)}`;

export const SEED_USERS: readonly SeedUser[] = [
  {
    key: "alice",
    id: "goldenUserAlice00000000000000001",
    username: "alice",
    password: "alice-password-1",
    apiKey: key("a1"),
    settings: {
      margin: 0.49,
      transferDay: 2.92,
      transferNight: 1.37,
      tax: 2.82752,
      vat: 25.5,
      nightStart: 22,
      nightEnd: 7,
      timezone: "Europe/Helsinki",
      area: "FI",
    },
  },
  {
    key: "bob",
    id: "goldenUserBob0000000000000000002",
    username: "bob",
    password: "bob-password-22",
    apiKey: key("b2"),
    settings: {
      margin: 0,
      transferDay: 4,
      transferNight: 2.5,
      tax: 4.39,
      vat: 25,
      nightStart: 0,
      nightEnd: 6,
      timezone: "Europe/Stockholm",
      area: "SE3",
    },
  },
  {
    key: "carol",
    id: "goldenUserCarol00000000000000003",
    username: "carol",
    password: "carol-password-3",
    apiKey: key("c3"),
    settings: {
      margin: 1.25,
      transferDay: 6.5,
      transferNight: 6.5,
      tax: 2.05,
      vat: 19,
      nightStart: 5,
      nightEnd: 5,
      timezone: "Europe/Berlin",
      area: "GER",
    },
  },
  {
    key: "dave",
    id: "goldenUserDave000000000000000004",
    username: "dave",
    password: "dave-password-44",
    apiKey: key("d4"),
    settings: {
      margin: 0.3,
      transferDay: 3.33,
      transferNight: 1.11,
      tax: 2.79372,
      vat: 24,
      nightStart: 23,
      nightEnd: 0,
      timezone: "Europe/Helsinki",
      area: "EE",
    },
  },
  {
    key: "eve",
    id: "goldenUserEve0000000000000000005",
    username: "eve",
    password: "eve-password-555",
    apiKey: key("e5"),
    settings: {
      margin: 0,
      transferDay: 0,
      transferNight: 0,
      tax: 0,
      vat: 0,
      nightStart: 22,
      nightEnd: 6,
      timezone: "Europe/Stockholm",
      area: "FI",
    },
  },
  {
    key: "frank",
    id: "goldenUserFrank00000000000000006",
    username: "frank",
    password: "frank-password-6",
    apiKey: key("f6"),
    settings: {
      margin: 0.1,
      transferDay: 1,
      transferNight: 1,
      tax: 1,
      vat: 25,
      nightStart: 22,
      nightEnd: 7,
      timezone: "Europe/Oslo",
      area: "NO1",
    },
  },
  {
    key: "grace",
    id: "goldenUserGrace00000000000000007",
    username: "grace",
    password: "grace-password-7",
    apiKey: key("07"),
    settings: null,
  },
];

export const AUTH_SECRET = "golden-http-fixture-secret-0123456789abcdef";

export interface SeedSession {
  readonly id: string;
  readonly token: string;
  readonly userKey: string;
  readonly createdAt: string;
  readonly expiresAt: string;
}

/** Sessions in the shared seed. Their tokens are fixed, so the signed cookie
 *  of each one is a deterministic test vector. */
export const SEED_SESSIONS: readonly SeedSession[] = [
  {
    id: "goldenSessionAliceValid000000001",
    token: "goldenTokenAliceValid00000000001",
    userKey: "alice",
    createdAt: "2026-10-24T08:00:00.000Z",
    expiresAt: "2026-10-31T08:00:00.000Z",
  },
  {
    id: "goldenSessionAliceExpired0000002",
    token: "goldenTokenAliceExpired000000002",
    userKey: "alice",
    createdAt: "2026-09-24T08:00:00.000Z",
    expiresAt: "2026-10-01T08:00:00.000Z",
  },
  {
    id: "goldenSessionGraceValid000000003",
    token: "goldenTokenGraceValid00000000003",
    userKey: "grace",
    createdAt: "2026-10-24T08:00:00.000Z",
    expiresAt: "2026-10-31T08:00:00.000Z",
  },
  {
    id: "goldenSessionBobOld0000000000004",
    token: "goldenTokenBobOld000000000000004",
    userKey: "bob",
    createdAt: "2026-10-20T08:00:00.000Z",
    expiresAt: "2026-10-27T08:00:00.000Z",
  },
];

export const userByKey = (k: string): SeedUser => {
  const u = SEED_USERS.find((x) => x.key === k);
  if (!u) {
    throw new Error(`unknown seed user ${k}`);
  }
  return u;
};

const lit = (s: string): string => `'${s.replace(/'/g, "''")}'`;

const USER_CREATED_AT = "2026-09-01T08:00:00.000Z";

const userSql = (u: SeedUser, hash: string, index: number): string => {
  const created = new Date(ms(USER_CREATED_AT) + index * HOUR_MS).toISOString();
  const lines = [
    `INSERT INTO "user" ("id", "name", "email", "emailVerified", "image", "createdAt", "updatedAt") VALUES (${lit(u.id)}, ${lit(u.username)}, ${lit(`${u.username}@local.spot`)}, false, NULL, ${lit(created)}, ${lit(created)});`,
    `INSERT INTO "account" ("id", "accountId", "providerId", "userId", "password", "createdAt", "updatedAt") VALUES (${lit(`goldenAccount${String(index + 1).padStart(19, "0")}`)}, ${lit(u.id)}, 'credential', ${lit(u.id)}, ${lit(hash)}, ${lit(created)}, ${lit(created)});`,
    `INSERT INTO usernames (user_id, username, created_at) VALUES (${lit(u.id)}, ${lit(u.username)}, ${lit(created)});`,
    `INSERT INTO api_keys (id, user_id, key_plaintext, created_at, last_used_at) VALUES (${lit(`goldenkey${String(index + 1).padStart(23, "0")}`)}, ${lit(u.id)}, ${lit(u.apiKey)}, ${lit(created)}, NULL);`,
  ];
  const s = u.settings;
  if (s) {
    lines.push(
      `INSERT INTO user_settings (user_id, margin_cents_kwh, transfer_day_cents_kwh, transfer_night_cents_kwh, tax_cents_kwh, vat_percent, night_start_hour, night_end_hour, timezone, area, updated_at) VALUES (${lit(u.id)}, ${String(s.margin)}, ${String(s.transferDay)}, ${String(s.transferNight)}, ${String(s.tax)}, ${String(s.vat)}, ${String(s.nightStart)}, ${String(s.nightEnd)}, ${lit(s.timezone)}, ${lit(s.area)}, ${lit(created)});`,
    );
  }
  return lines.join("\n");
};

// Fingrid coverage. Actuals stop at the latest seeded "now"; the forecast
// vintages reach past the forecast series end (2026-10-28T22:00:00Z).
const ACTUALS_FROM = "2026-09-20T00:00:00Z";
const ACTUALS_TO = "2026-10-24T09:00:00Z";
const VINTAGES = [
  {
    issuedAt: "2026-10-20T12:00:00Z",
    from: "2026-09-20T00:00:00Z",
    to: "2026-10-29T00:00:00Z",
    shift: 0,
  },
  {
    issuedAt: "2026-10-24T08:00:00Z",
    from: "2026-10-24T02:00:00Z",
    to: "2026-10-29T00:00:00Z",
    shift: 150,
  },
] as const;

export const buildSeedSql = (hashes: ReadonlyMap<string, string>): string => {
  lcg = 20261024;
  const parts: string[] = [
    "-- Golden HTTP seed. Generated by scripts/golden/http.ts; do not edit.",
    "-- Load into a freshly migrated, empty database.",
    "BEGIN;",
  ];

  for (const w of PRICE_WINDOWS) {
    const ts = quarters(w.from, w.to);
    const values = ts.map((t) => {
      const iso = new Date(t).toISOString().replace(".000Z", "Z");
      const o = w.overrides?.[iso];
      const base = priceFrom(t, w.bias);
      return o === undefined ? base : o;
    });
    parts.push(
      `-- prices ${w.area} ${w.from} .. ${w.to}`,
      quarterRun(
        "prices",
        "delivery_start, delivery_end, price_eur_mwh, area",
        `, '${w.area}'`,
        w.from,
        values,
      ),
    );
  }

  const actualTs = quarters(ACTUALS_FROM, ACTUALS_TO);
  parts.push(
    "-- fingrid_actuals 75 (wind) and 124 (consumption)",
    quarterRun(
      "fingrid_actuals",
      "start_time, end_time, value, dataset_id",
      ", 75",
      ACTUALS_FROM,
      actualTs.map(wind),
    ),
    quarterRun(
      "fingrid_actuals",
      "start_time, end_time, value, dataset_id",
      ", 124",
      ACTUALS_FROM,
      actualTs.map(consumption),
    ),
  );
  for (const v of VINTAGES) {
    const ts = quarters(v.from, v.to);
    parts.push(
      `-- fingrid_forecasts 245 and 165 issued ${v.issuedAt}`,
      quarterRun(
        "fingrid_forecasts",
        "start_time, end_time, value, dataset_id, issued_at",
        `, 245, timestamptz '${v.issuedAt}'`,
        v.from,
        ts.map((t) => r1(wind(t) + v.shift)),
      ),
      quarterRun(
        "fingrid_forecasts",
        "start_time, end_time, value, dataset_id, issued_at",
        `, 165, timestamptz '${v.issuedAt}'`,
        v.from,
        ts.map((t) => r1(consumption(t) - v.shift)),
      ),
    );
  }

  SEED_USERS.forEach((u, i) => {
    const hash = hashes.get(u.key);
    if (!hash) {
      throw new Error(`missing hash for ${u.key}`);
    }
    parts.push(`-- user ${u.username}`, userSql(u, hash, i));
  });

  for (const s of SEED_SESSIONS) {
    const u = userByKey(s.userKey);
    parts.push(
      `INSERT INTO "session" ("id", "expiresAt", "token", "createdAt", "updatedAt", "ipAddress", "userAgent", "userId") VALUES (${lit(s.id)}, ${lit(s.expiresAt)}, ${lit(s.token)}, ${lit(s.createdAt)}, ${lit(s.createdAt)}, NULL, NULL, ${lit(u.id)});`,
    );
  }

  parts.push("COMMIT;", "");
  return parts.join("\n");
};
