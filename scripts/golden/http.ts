// Writes the HTTP golden fixtures of the Rust server from the real TypeScript
// service in src/. The Rust tests in crates/server/tests/ compare against them.
//
// Run: node --import tsx scripts/golden/http.ts
// Needs the isolated PostgreSQL of scripts/test-db.sh (GOLDEN_PG_URL, default
// postgres://postgres@127.0.0.1:55436). The run creates and drops its own
// golden_http_* databases and never touches another database.
//
// Each case runs in its own child process (fresh rate-limit stores) against
// its own copy of the seeded template database. The clock is pinned and every
// random source is seeded, so two runs write identical files.
import { spawn } from "node:child_process";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { installDeterminism, reseed, setNow } from "./http/determinism.js";

const SCRIPT = fileURLToPath(import.meta.url);
const ROOT = join(dirname(SCRIPT), "../..");
const OUT_DIR = join(ROOT, "crates/server/tests/golden/http");
const PG_BASE =
  process.env["GOLDEN_PG_URL"] ?? "postgres://postgres@127.0.0.1:55436";
const TEMPLATE_DB = "golden_http_template";
const CONCURRENCY = 6;
const INLINE_BODY_LIMIT = 2048;
const VOLATILE = "<volatile>";

const PROFILES = {
  test: { NODE_ENV: "test", BETTER_AUTH_URL: "http://localhost:3000" },
  production: {
    NODE_ENV: "production",
    BETTER_AUTH_URL: "https://spot.calmdonut.com",
  },
} as const;

const baseEnv = (
  profile: keyof typeof PROFILES,
  db: string,
): NodeJS.ProcessEnv => {
  const env: NodeJS.ProcessEnv = {
    PATH: process.env["PATH"] ?? "",
    HOME: process.env["HOME"] ?? "",
    TZ: "UTC",
    LANG: "C",
    DATABASE_URL: `${PG_BASE}/${db}`,
    BETTER_AUTH_SECRET: "",
    ...PROFILES[profile],
  };
  return env;
};

// --- Worker side ---------------------------------------------------------------

interface RawResponse {
  readonly status: number;
  readonly headers: [string, string][];
  readonly setCookie: string[];
  readonly body: string;
}

interface RawStep {
  readonly clock: string;
  readonly response: RawResponse;
  readonly repeatStatuses?: number[];
}

const PLACEHOLDER = /\{\{s(\d+)\.(json|cookie):([^}]+)\}\}/g;

const jsonPointerGet = (doc: unknown, pointer: string): unknown => {
  let cur: unknown = doc;
  for (const raw of pointer.split("/").slice(1)) {
    const key = raw.replace(/~1/g, "/").replace(/~0/g, "~");
    if (cur === null || typeof cur !== "object") {
      return undefined;
    }
    cur = (cur as Record<string, unknown>)[key];
  }
  return cur;
};

const jsonPointerSet = (
  doc: unknown,
  pointer: string,
  value: unknown,
): void => {
  const parts = pointer
    .split("/")
    .slice(1)
    .map((p) => p.replace(/~1/g, "/").replace(/~0/g, "~"));
  const last = parts.pop();
  let cur: unknown = doc;
  for (const key of parts) {
    cur = (cur as Record<string, unknown>)[key];
  }
  if (last === undefined || cur === null || typeof cur !== "object") {
    throw new Error(`pointer ${pointer} has no parent`);
  }
  (cur as Record<string, unknown>)[last] = value;
};

const cookiePair = (setCookie: string): [string, string] => {
  const first = setCookie.split(";")[0] ?? "";
  const eq = first.indexOf("=");
  return [first.slice(0, eq), first.slice(eq + 1)];
};

const resolvePlaceholders = (text: string, done: readonly RawStep[]): string =>
  text.replace(PLACEHOLDER, (_m, idx: string, kind: string, arg: string) => {
    const step = done[Number(idx)];
    if (!step) {
      throw new Error(`placeholder refers to a later step: ${text}`);
    }
    if (kind === "json") {
      const v = jsonPointerGet(JSON.parse(step.response.body), arg);
      if (typeof v !== "string") {
        throw new Error(`placeholder ${arg} is not a string`);
      }
      return v;
    }
    const match = step.response.setCookie
      .map(cookiePair)
      .find(([n]) => n === arg);
    if (!match) {
      throw new Error(`step ${idx} set no cookie ${arg}`);
    }
    return `${match[0]}=${match[1]}`;
  });

const runWorker = async (caseName: string, outFile: string): Promise<void> => {
  installDeterminism();
  const { CASES } = await import("./http/cases.js");
  const def = CASES.find((c) => c.name === caseName);
  if (!def) {
    throw new Error(`unknown case ${caseName}`);
  }
  reseed(def.name);
  setNow(def.clock);

  // db.js installs the TIMESTAMPTZ type parser the service relies on.
  await import("../../src/db.js");
  const pg = (await import("pg")).default;
  const { createAuth } = await import("../../src/auth.js");
  const { createApp } = await import("../../src/app.js");

  const pool = new pg.Pool({ connectionString: process.env["DATABASE_URL"] });
  const app = createApp(pool, createAuth(pool));

  const done: RawStep[] = [];
  let clock = def.clock;
  for (const step of def.steps) {
    if (step.clock) {
      if (Date.parse(step.clock) < Date.parse(clock)) {
        throw new Error(`${def.name}: step clocks must not go backwards`);
      }
      clock = step.clock;
    }
    setNow(clock);
    if (step.special === "close-db") {
      await pool.end();
    }
    const headers = new Headers();
    for (const [k, v] of Object.entries(step.request.headers ?? {})) {
      headers.set(k, resolvePlaceholders(v, done));
    }
    const init: RequestInit = { method: step.request.method, headers };
    if (step.request.body !== undefined) {
      init.body = step.request.body;
    }
    const times = step.repeat ?? 1;
    const statuses: number[] = [];
    let last: RawResponse | undefined;
    for (let i = 0; i < times; i++) {
      const res = await app.request(step.request.path, init);
      const body = await res.text();
      statuses.push(res.status);
      last = {
        status: res.status,
        headers: [...res.headers.entries()].filter(([k]) => k !== "set-cookie"),
        setCookie: res.headers.getSetCookie(),
        body,
      };
    }
    if (!last) {
      throw new Error("repeat must be at least 1");
    }
    done.push({
      clock,
      response: last,
      ...(step.repeat ? { repeatStatuses: statuses } : {}),
    });
  }
  if (!def.steps.some((s) => s.special === "close-db")) {
    await pool.end();
  }
  writeFileSync(outFile, JSON.stringify(done));
};

// --- Auth vectors (worker side) --------------------------------------------------

const VECTOR_PASSWORDS: readonly {
  label: string;
  password: string;
  others: string[];
}[] = [
  {
    label: "ascii",
    password: "password123",
    others: ["password124", "Password123", ""],
  },
  { label: "ascii-8-min-length", password: "12345678", others: ["1234567"] },
  {
    label: "ascii-128-max-length",
    password: "p".repeat(128),
    others: ["p".repeat(127)],
  },
  { label: "ascii-7-below-min", password: "1234567", others: [] },
  {
    label: "fullwidth-nfkc",
    password: "ｐａｓｓｗｏｒｄ１２",
    others: ["password12"],
  },
  {
    label: "composed-e-acute",
    password: "café-pass",
    others: ["café-pass", "cafe-pass"],
  },
  { label: "decomposed-e-acute", password: "café-pass", others: ["café-pass"] },
  {
    label: "ligature-fi-nfkc-changes-length",
    password: "ﬁﬁﬁﬁﬁﬁﬁﬁ",
    others: ["fifififififififi"],
  },
  { label: "emoji-astral", password: "🔑🔑🔑🔑", others: ["🔑🔑🔑"] },
  {
    label: "spaces-and-kana",
    password: " パスワード 1234 ",
    others: ["パスワード 1234"],
  },
];

const runAuthVectors = async (outFile: string): Promise<void> => {
  installDeterminism();
  const T_AM = "2026-10-24T09:07:30.000Z";
  setNow(T_AM);
  await import("../../src/db.js");
  const pg = (await import("pg")).default;
  const { hashPassword, verifyPassword } = await import("better-auth/crypto");
  const { createAuth } = await import("../../src/auth.js");
  const { createApp } = await import("../../src/app.js");
  const { signedCookieValue } = await import("./http/cases.js");
  const { SEED_SESSIONS, AUTH_SECRET } = await import("./http/seed.js");

  const passwordVectors = [];
  for (const [i, v] of VECTOR_PASSWORDS.entries()) {
    reseed(`auth-vector-${String(i)}`);
    const hash = await hashPassword(v.password);
    const checks = [];
    for (const candidate of [v.password, ...v.others]) {
      checks.push({
        candidate,
        result: await verifyPassword({ hash, password: candidate }),
      });
    }
    passwordVectors.push({
      label: v.label,
      password: v.password,
      utf16Length: v.password.length,
      codePoints: [...v.password].length,
      nfkc: v.password.normalize("NFKC"),
      hash,
      verify: checks,
    });
  }

  const pool = new pg.Pool({ connectionString: process.env["DATABASE_URL"] });
  const auth = createAuth(pool);
  const app = createApp(pool, auth);

  reseed("auth-vector-signup");
  const signupRes = await app.request("/api/session/login-or-signup", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      username: "vectoruser",
      password: "vector-password-1",
    }),
  });
  const signupBody = await signupRes.text();
  const userId = (JSON.parse(signupBody) as { user: { id: string } }).user.id;
  const rows = async (sql: string): Promise<unknown[]> =>
    (await pool.query(sql, [userId])).rows;
  const signupRows = {
    user: await rows(`SELECT * FROM "user" WHERE id = $1`),
    account: await rows(`SELECT * FROM "account" WHERE "userId" = $1`),
    session: await rows(`SELECT * FROM "session" WHERE "userId" = $1`),
    // created_at comes from the database default NOW(), the real wall clock.
    usernames: (await rows(`SELECT * FROM usernames WHERE user_id = $1`)).map(
      (r) => ({
        ...(r as Record<string, unknown>),
        created_at: "<volatile: database NOW()>",
      }),
    ),
    user_settings: await rows(`SELECT * FROM user_settings WHERE user_id = $1`),
    api_keys: await rows(`SELECT * FROM api_keys WHERE user_id = $1`),
  };
  const sessionRow = signupRows.session[0] as { token: string } | undefined;
  const setCookie = signupRes.headers.getSetCookie();

  const call = async (label: string, fn: () => Promise<Response>) => {
    reseed(`auth-vector-${label}`);
    const res = await fn();
    return {
      label,
      status: res.status,
      headers: Object.fromEntries(
        [...res.headers.entries()].filter(([k]) => k !== "set-cookie"),
      ),
      setCookie: res.headers.getSetCookie(),
      body: await res.text(),
    };
  };
  const h = (): Headers => new Headers({ "content-type": "application/json" });
  const direct = [
    await call("signUpEmail-too-short", () =>
      auth.api.signUpEmail({
        headers: h(),
        body: { email: "short@local.spot", password: "1234567", name: "short" },
        asResponse: true,
      }),
    ),
    await call("signUpEmail-too-long", () =>
      auth.api.signUpEmail({
        headers: h(),
        body: {
          email: "long@local.spot",
          password: "p".repeat(129),
          name: "long",
        },
        asResponse: true,
      }),
    ),
    await call("signUpEmail-existing-email", () =>
      auth.api.signUpEmail({
        headers: h(),
        body: {
          email: "alice@local.spot",
          password: "another-password",
          name: "alice",
        },
        asResponse: true,
      }),
    ),
    await call("signUpEmail-invalid-email", () =>
      auth.api.signUpEmail({
        headers: h(),
        body: {
          email: "not-an-email",
          password: "another-password",
          name: "x",
        },
        asResponse: true,
      }),
    ),
    await call("signUpEmail-autoSignIn", () =>
      auth.api.signUpEmail({
        headers: h(),
        body: {
          email: "auto@local.spot",
          password: "auto-password-1",
          name: "auto",
        },
        asResponse: true,
      }),
    ),
    await call("signInEmail-wrong-password", () =>
      auth.api.signInEmail({
        headers: h(),
        body: {
          email: "alice@local.spot",
          password: "wrong-password",
          rememberMe: true,
        },
        asResponse: true,
      }),
    ),
    await call("signInEmail-unknown-email", () =>
      auth.api.signInEmail({
        headers: h(),
        body: {
          email: "nobody@local.spot",
          password: "wrong-password",
          rememberMe: true,
        },
        asResponse: true,
      }),
    ),
    await call("signInEmail-short-password", () =>
      auth.api.signInEmail({
        headers: h(),
        body: {
          email: "alice@local.spot",
          password: "short",
          rememberMe: true,
        },
        asResponse: true,
      }),
    ),
    await call("signInEmail-correct", () =>
      auth.api.signInEmail({
        headers: h(),
        body: {
          email: "alice@local.spot",
          password: "alice-password-1",
          rememberMe: true,
        },
        asResponse: true,
      }),
    ),
    await call("signInEmail-rememberMe-false", () =>
      auth.api.signInEmail({
        headers: h(),
        body: {
          email: "alice@local.spot",
          password: "alice-password-1",
          rememberMe: false,
        },
        asResponse: true,
      }),
    ),
    await call("signOut-without-cookie", () =>
      auth.api.signOut({ headers: new Headers(), asResponse: true }),
    ),
  ];

  const cookieVectors = SEED_SESSIONS.map((s) => ({
    token: s.token,
    secret: AUTH_SECRET,
    cookieValue: signedCookieValue(s.token),
  }));

  await pool.end();
  writeFileSync(
    outFile,
    JSON.stringify({
      passwordVectors,
      signup: {
        clock: T_AM,
        request: { username: "vectoruser", password: "vector-password-1" },
        response: { status: signupRes.status, setCookie, body: signupBody },
        rows: signupRows,
        sessionTokenFromDb: sessionRow?.token ?? null,
        cookieMatchesSignedToken: sessionRow
          ? setCookie.some(
              (c) => cookiePair(c)[1] === signedCookieValue(sessionRow.token),
            )
          : false,
      },
      direct,
      cookieVectors,
    }),
  );
};

// --- Orchestrator ------------------------------------------------------------------

const runChild = (args: string[], env: NodeJS.ProcessEnv): Promise<void> =>
  new Promise((resolve, reject) => {
    const child = spawn(
      process.execPath,
      ["--import", "tsx", SCRIPT, ...args],
      {
        cwd: ROOT,
        env,
        stdio: ["ignore", "ignore", "pipe"],
      },
    );
    let stderr = "";
    child.stderr.on("data", (d: Buffer) => {
      stderr += d.toString();
    });
    child.on("exit", (code) => {
      if (code === 0) {
        resolve();
      } else {
        reject(
          new Error(
            `worker ${args.join(" ")} failed (${String(code)}):\n${stderr}`,
          ),
        );
      }
    });
  });

const orchestrate = async (): Promise<void> => {
  installDeterminism();
  setNow("2026-10-24T09:07:30.000Z");
  const pg = (await import("pg")).default;
  const { runMigrations } = await import("../../src/migrate.js");
  const { hashPassword } = await import("better-auth/crypto");
  const { SEED_USERS, buildSeedSql, AUTH_SECRET } =
    await import("./http/seed.js");
  const { CASES } = await import("./http/cases.js");

  const admin = new pg.Pool({
    connectionString: `${PG_BASE}/postgres`,
    max: 2,
  });
  const recreate = async (db: string, template?: string): Promise<void> => {
    await admin.query(`DROP DATABASE IF EXISTS ${db} WITH (FORCE)`);
    await admin.query(
      `CREATE DATABASE ${db}${template ? ` TEMPLATE ${template}` : ""}`,
    );
  };
  const drop = async (db: string): Promise<void> => {
    await admin.query(`DROP DATABASE IF EXISTS ${db} WITH (FORCE)`);
  };

  // Seed users get real Better Auth hashes with a seeded salt.
  const hashes = new Map<string, string>();
  for (const u of SEED_USERS) {
    reseed(`seed-user-${u.key}`);
    hashes.set(u.key, await hashPassword(u.password));
  }
  const seedSql = buildSeedSql(hashes);

  await recreate(TEMPLATE_DB);
  {
    const pool = new pg.Pool({ connectionString: `${PG_BASE}/${TEMPLATE_DB}` });
    // runMigrations logs each applied file; keep the generator output short.
    const log = console.log;
    console.log = () => undefined;
    await runMigrations(pool);
    console.log = log;
    await pool.query(seedSql);
    await pool.end();
  }

  // README.md is hand-written; every other file here is generated.
  rmSync(join(OUT_DIR, "bodies"), { recursive: true, force: true });
  mkdirSync(join(OUT_DIR, "bodies"), { recursive: true });
  writeFileSync(join(OUT_DIR, "seed.sql"), seedSql);

  const tmp = join(ROOT, ".tmp/golden-http");
  rmSync(tmp, { recursive: true, force: true });
  mkdirSync(tmp, { recursive: true });

  const raw = new Map<string, RawStep[]>();
  // GOLDEN_ONLY=<regex> limits a development run to matching cases.
  const only = process.env["GOLDEN_ONLY"];
  const selected = only
    ? CASES.filter((c) => new RegExp(only).test(c.name))
    : CASES;
  const queue = selected.map((c, i) => ({ c, i }));
  const workers = Array.from({ length: CONCURRENCY }, async () => {
    for (;;) {
      const item = queue.shift();
      if (!item) {
        return;
      }
      const db = `golden_http_case_${String(item.i)}`;
      await recreate(db, TEMPLATE_DB);
      if (item.c.setupSql) {
        const pool = new pg.Pool({ connectionString: `${PG_BASE}/${db}` });
        await pool.query(item.c.setupSql);
        await pool.end();
      }
      const out = join(tmp, `${item.c.name}.json`);
      const env = baseEnv(item.c.profile ?? "test", db);
      env["BETTER_AUTH_SECRET"] = AUTH_SECRET;
      await runChild(["--worker", item.c.name, out], env);
      raw.set(item.c.name, JSON.parse(readFileSync(out, "utf8")) as RawStep[]);
      await drop(db);
      process.stderr.write(".");
    }
  });
  await Promise.all(workers);
  process.stderr.write("\n");

  // Auth vectors use their own database copy.
  const vectorDb = "golden_http_vectors";
  await recreate(vectorDb, TEMPLATE_DB);
  const vectorOut = join(tmp, "auth-vectors.json");
  const vectorEnv = baseEnv("test", vectorDb);
  vectorEnv["BETTER_AUTH_SECRET"] = AUTH_SECRET;
  await runChild(["--auth-vectors", vectorOut], vectorEnv);
  await drop(vectorDb);
  await drop(TEMPLATE_DB);
  await admin.end();

  await writeCases(selected, raw);
  await writeAuthVectors(
    JSON.parse(readFileSync(vectorOut, "utf8")) as AuthVectorRaw,
  );
  rmSync(tmp, { recursive: true, force: true });
};

// --- Normalisation and output ------------------------------------------------------

type Cases = (typeof import("./http/cases.js"))["CASES"];

const extFor = (contentType: string | undefined): string =>
  contentType?.includes("json")
    ? "json"
    : contentType?.includes("html")
      ? "html"
      : "txt";

/** Writes JSON through Prettier, so `pnpm format:check` accepts the file. */
const writeJson = async (name: string, value: unknown): Promise<void> => {
  const prettier = await import("prettier");
  const path = join(OUT_DIR, name);
  const text = await prettier.format(JSON.stringify(value), {
    parser: "json",
    filepath: path,
    ...(await prettier.resolveConfig(path)),
  });
  writeFileSync(path, text);
};

const writeCases = async (
  cases: Cases,
  raw: ReadonlyMap<string, RawStep[]>,
): Promise<void> => {
  const out = [];
  let openapi: unknown = null;
  const counts = new Map<string, number>();
  for (const def of cases) {
    const steps = raw.get(def.name);
    if (!steps) {
      throw new Error(`no result for ${def.name}`);
    }
    counts.set(def.group, (counts.get(def.group) ?? 0) + 1);
    const secrets = new Set<string>();
    const outSteps = def.steps.map((step, idx) => {
      const r = steps[idx];
      if (!r) {
        throw new Error(`${def.name}: missing step ${String(idx)}`);
      }
      let body = r.response.body;
      if (step.volatile && step.volatile.length > 0) {
        const doc: unknown = JSON.parse(body);
        if (JSON.stringify(doc) !== body) {
          throw new Error(
            `${def.name}#${String(idx)}: body does not round-trip through JSON`,
          );
        }
        for (const v of step.volatile) {
          const value = jsonPointerGet(doc, v.pointer);
          if (typeof value !== "string" || !new RegExp(v.pattern).test(value)) {
            throw new Error(
              `${def.name}#${String(idx)}: ${v.pointer} = ${JSON.stringify(value)} does not match ${v.pattern}`,
            );
          }
          secrets.add(value);
          jsonPointerSet(doc, v.pointer, VOLATILE);
        }
        body = JSON.stringify(doc);
      }
      const setCookie = r.response.setCookie.map((c) => {
        const [name, value] = cookiePair(c);
        if (value === "") {
          return c;
        }
        secrets.add(value);
        secrets.add(decodeURIComponent(value).split(".")[0] ?? value);
        return `${name}=${VOLATILE}${c.slice(c.indexOf(";") === -1 ? c.length : c.indexOf(";"))}`;
      });
      const headers = Object.fromEntries(r.response.headers);
      const contentType = headers["content-type"];
      const response: Record<string, unknown> = {
        status: r.response.status,
        headers,
        setCookie,
      };
      if (
        def.name === "openapi-and-docs" &&
        step.request.path === "/api/v1/openapi.json"
      ) {
        openapi = JSON.parse(body);
      }
      if (body.length > INLINE_BODY_LIMIT) {
        // The ".body" suffix keeps Prettier away from the exact bytes.
        const file = `bodies/${def.name}.${String(idx)}.${extFor(contentType)}.body`;
        writeFileSync(join(OUT_DIR, file), body);
        response["bodyFile"] = file;
      } else {
        response["body"] = body;
      }
      return {
        ...(r.clock !== def.clock ? { clock: r.clock } : {}),
        request: step.request,
        ...(step.repeat
          ? {
              repeat: step.repeat,
              repeatStatuses: runLength(r.repeatStatuses ?? []),
            }
          : {}),
        ...(step.special ? { special: step.special } : {}),
        ...(step.note ? { note: step.note } : {}),
        response,
        volatile: step.volatile ?? [],
      };
    });
    // Every random value captured in this case must be masked everywhere.
    const serialised = JSON.stringify(outSteps);
    for (const s of secrets) {
      if (s.length >= 8 && serialised.includes(s)) {
        throw new Error(
          `${def.name}: volatile value ${s} leaked into the fixture`,
        );
      }
    }
    out.push({
      name: def.name,
      group: def.group,
      clock: def.clock,
      profile: def.profile ?? "test",
      ...(def.setupSql ? { setupSql: def.setupSql } : {}),
      ...(def.note ? { note: def.note } : {}),
      steps: outSteps,
    });
  }
  await writeJson("cases.json", out);
  if (openapi !== null) {
    await writeJson("openapi.ts.json", openapi);
  }
  process.stderr.write(
    `cases: ${String(out.length)}; steps: ${String(out.reduce((n, c) => n + c.steps.length, 0))}\n`,
  );
  for (const [g, n] of counts) {
    process.stderr.write(`  ${g}: ${String(n)}\n`);
  }
};

const runLength = (
  statuses: readonly number[],
): { status: number; count: number }[] => {
  const out: { status: number; count: number }[] = [];
  for (const s of statuses) {
    const last = out[out.length - 1];
    if (last && last.status === s) {
      last.count++;
    } else {
      out.push({ status: s, count: 1 });
    }
  }
  return out;
};

interface AuthVectorRaw {
  readonly [key: string]: unknown;
}

const writeAuthVectors = async (v: AuthVectorRaw): Promise<void> => {
  await writeJson("auth-vectors.json", {
    note: "Values come from the real Better Auth code with a seeded random source; ids, salts and tokens are examples of the format, not values the Rust service must reproduce.",
    ...v,
  });
};

// --- Entry ---------------------------------------------------------------------------

const [mode, a1, a2] = process.argv.slice(2);
if (mode === "--worker" && a1 && a2) {
  await runWorker(a1, a2);
} else if (mode === "--auth-vectors" && a1) {
  await runAuthVectors(a1);
} else if (mode === undefined) {
  await orchestrate();
} else {
  throw new Error(
    "usage: http.ts [--worker <case> <out> | --auth-vectors <out>]",
  );
}
