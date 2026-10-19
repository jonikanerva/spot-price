# HTTP golden fixtures

These fixtures record the HTTP responses of the real TypeScript service. The
Rust HTTP tests compare the Rust service against them. Do not edit the
generated files by hand.

The TypeScript service and the generator (`scripts/golden/http.ts` and
`scripts/golden/http/*.ts`) are no longer in the tree. They are in the Git
history of the Rust port PR: check out commit `e22c094` (the commit that
added these files) to generate them again.

## Generate (at commit `e22c094`)

```sh
scripts/test-db.sh start
node --import tsx scripts/golden/http.ts
```

- Run the command outside the sandbox. The generator connects to PostgreSQL.
- `GOLDEN_PG_URL` selects the server. The default is
  `postgres://postgres@127.0.0.1:55436`.
- The generator creates `golden_http_template`, one `golden_http_case_<n>`
  database for each case, and `golden_http_vectors`. It drops all of them at
  the end. It does not touch another database.
- `GOLDEN_ONLY=<regex>` runs only the matching cases. Use it only during
  development: the run replaces the whole output directory.
- Two runs write identical files. This was checked with `diff -r`.

Recorded environment of the committed files:

| Item                       | Value                                                               |
| -------------------------- | ------------------------------------------------------------------- |
| TypeScript source          | commit `5b79d4a653a9d8f115499f0b5491307d968dab9e`, `src/` unchanged |
| Node.js                    | v24.21.0                                                            |
| PostgreSQL                 | 17.11                                                               |
| hono                       | 4.13.13                                                             |
| @hono/zod-openapi          | 1.4.0                                                               |
| zod                        | 4.4.3                                                               |
| better-auth                | 1.6.33                                                              |
| hono-rate-limiter          | 0.5.3                                                               |
| @scalar/hono-api-reference | 0.10.20                                                             |
| pg                         | 8.20.0                                                              |

## Files

| File                | Content                                                                                                                       |
| ------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| `seed.sql`          | The shared seed. Load it into an empty, migrated database.                                                                    |
| `cases.json`        | The request and response cases.                                                                                               |
| `bodies/*.body`     | Response bodies longer than 2048 characters, byte for byte.                                                                   |
| `openapi.ts.json`   | The OpenAPI document of the TypeScript service, pretty-printed. The exact bytes are in `bodies/openapi-and-docs.0.json.body`. |
| `auth-vectors.json` | Better Auth password hashes, sign-up rows, and error bodies.                                                                  |

## How one case runs

The Rust harness must reproduce these conditions:

1. Create a fresh database with the schema. Load `seed.sql`. Then run the
   `setupSql` of the case, if the case has one.
2. Start a fresh service instance. All rate-limit counters start empty.
3. Pin the clock to `clock` of the case. The clock does not move during a
   request. A step with its own `clock` moves the clock forward to that value
   before the step. Step clocks never go backwards.
4. Send the steps in order, one at a time.

The TypeScript run used these settings:

- `TZ=UTC`.
- `BETTER_AUTH_SECRET=golden-http-fixture-secret-0123456789abcdef`.
- Profile `test`: `NODE_ENV=test`, `BETTER_AUTH_URL=http://localhost:3000`.
- Profile `production`: `NODE_ENV=production`,
  `BETTER_AUTH_URL=https://spot.calmdonut.com`. This profile changes the
  session cookie name to `__Secure-better-auth.session_token` and adds the
  `Secure` attribute.
- No upstream API key is set. The scheduler does not run.
- Requests go through Hono `app.request()`, not through a socket. The request
  URL is `http://localhost<path>`. No client address exists, so the global rate
  limit uses the key `unknown` unless the request has `x-real-ip` or
  `x-forwarded-for`.
- The generator seeds every random source (`crypto.getRandomValues` and
  `node:crypto` `randomBytes`). Random values are therefore stable from run to
  run. They are still marked volatile, because the Rust service generates its
  own random values.

## Case schema

`cases.json` is an array of cases:

```jsonc
{
  "name": "session-signup-full-flow", // unique
  "group": "session", // basic, bearer-auth, price, price-now,
  // cheapest, history, forecast, public,
  // session, settings, ratelimit, production
  "clock": "2026-10-24T09:07:30.000Z", // the pinned clock of the case
  "profile": "test", // "test" or "production"
  "setupSql": "...", // optional, runs after seed.sql
  "note": "...", // optional
  "steps": [
    {
      "clock": "...", // optional, moves the clock forward
      "request": {
        "method": "GET",
        "path": "/api/v1/price/now?x=1", // path and raw query string
        "headers": { "authorization": "Bearer ..." }, // optional
        "body": "...", // optional, raw request body
      },
      "repeat": 119, // optional, see below
      "repeatStatuses": [{ "status": 200, "count": 119 }],
      "special": "close-db", // optional, see below
      "response": {
        "status": 200,
        "headers": {
          "content-type": "application/json",
          "ratelimit-limit": "60",
        },
        "setCookie": [
          "better-auth.session_token=<volatile>; Max-Age=604800; Path=/; HttpOnly; SameSite=Lax",
        ],
        "body": "{...}", // or "bodyFile": "bodies/<name>.<step>.json.body"
      },
      "volatile": [{ "pointer": "/token", "pattern": "^[A-Za-z0-9]{32}$" }],
    },
  ],
}
```

- `response.headers` holds every header of the response except `set-cookie`,
  with lowercase names. `app.request()` adds no transport headers, so `date`,
  `connection`, `keep-alive`, `content-length`, and `transfer-encoding` are not
  recorded. Compare only the recorded headers, and require that the Rust
  response has no other application header (for example a `ratelimit-*` header
  that the TypeScript response does not have).
- `response.setCookie` holds each `Set-Cookie` header in order.
- The JSON files are formatted by Prettier. A body is a JSON string inside
  them, so the formatting does not change it. The `.body` suffix keeps
  Prettier away from the body files.
- `response.body` or the file named by `response.bodyFile` holds the body
  exactly as the service sent it. A `HEAD` response has an empty body.
- `repeat: n` sends the same request `n` times. `repeatStatuses` lists the
  status of each send as runs of equal values. `response` is the response of
  the last send.
- `special: "close-db"` closes the database pool before the request. The Rust
  harness must make the database unreachable for that step.

### Placeholders in requests

A request header value can refer to an earlier step of the same case:

- `{{s<i>.json:<pointer>}}` is the string at the JSON pointer in the response
  body of step `i`. Example: `Bearer {{s2.json:/apiKey}}`.
- `{{s<i>.cookie:<name>}}` is `<name>=<value>` from the `Set-Cookie` header of
  step `i`. Example: `{{s0.cookie:better-auth.session_token}}`.

The harness uses the real values from its own responses, not the `<volatile>`
text in the fixture.

### Volatile values

- `volatile` lists JSON pointers into the response body. The generator
  replaced the value at each pointer with the string `"<volatile>"`. The real
  value matched `pattern`. To compare, parse the Rust body, check each value
  against `pattern`, set it to `"<volatile>"`, serialise it compactly, and
  compare the bytes. Without `volatile` entries, compare the raw bytes. The
  Rust harness also accepts the intentional differences that
  `golden_http.rs` lists. For example, the `forecast-values` kind accepts a
  forecast body that differs only in its `estimated*CentsKwh` numbers
  (`STACK.md` §2.1).
- In `setCookie`, every non-empty cookie value is `<volatile>`. The attributes
  are verbatim. An empty value (a cookie that is cleared) is verbatim.
- The generator fails when a captured random value appears anywhere else in
  the stored case.
- In `auth-vectors.json`, `usernames.created_at` is
  `"<volatile: database NOW()>"`. The column takes the database default, which
  is the real wall clock.

## Seed calendar

All prices are 15-minute rows. A range is `[from, to)` in UTC.

| Area | Range (UTC)                            | Meaning                                                  |
| ---- | -------------------------------------- | -------------------------------------------------------- |
| FI   | 2026-03-26T00:00Z .. 2026-03-31T00:00Z | Spring DST days                                          |
| SE3  | 2026-03-26T00:00Z .. 2026-03-31T00:00Z | Spring DST days                                          |
| GER  | 2026-03-26T00:00Z .. 2026-03-31T00:00Z | Spring DST days                                          |
| EE   | 2026-03-26T00:00Z .. 2026-03-31T00:00Z | Spring DST days                                          |
| FI   | 2026-09-20T00:00Z .. 2026-10-25T22:00Z | Ends with local 2026-10-25 (25 hours). Forecast history. |
| SE1  | 2026-09-20T00:00Z .. 2026-10-25T23:00Z | Forecast neighbour                                       |
| SE3  | 2026-10-20T00:00Z .. 2026-10-24T22:00Z | Local 2026-10-25 is not published                        |
| EE   | 2026-10-20T00:00Z .. 2026-10-25T07:00Z | Only 40 quarters of local 2026-10-25 are published       |
| GER  | 2026-10-20T22:00Z .. 2026-10-25T23:00Z | Local 2026-10-21 .. 2026-10-25 complete                  |
| NO1  | none                                   | No data                                                  |

FI special values (EUR/MWh) on the night of local 2026-10-24:
`22:00Z -1.5`, `22:15Z -0.01`, `22:30Z 0`, `22:45Z -0` (negative zero),
`23:00Z -12.34`, `23:15Z -500`, `23:30Z 0`, `23:45Z 0.01` (all 2026-10-23).
FI has a gap at 2026-10-24T15:00Z .. 16:00Z, a spike of 999.99 at
2026-10-24T16:00Z, and a cheap block 1.11 .. 1.14 at 2026-10-25T02:00Z .. 03:00Z.

Fingrid rows:

- `fingrid_actuals` 75 and 124: 2026-09-20T00:00Z .. 2026-10-24T09:00Z.
- `fingrid_forecasts` 245 and 165, issued 2026-10-20T12:00Z:
  2026-09-20T00:00Z .. 2026-10-29T00:00Z.
- `fingrid_forecasts` 245 and 165, issued 2026-10-24T08:00Z (different
  values): 2026-10-24T02:00Z .. 2026-10-29T00:00Z.

The forecast series starts one quarter after the latest FI price in the whole
table, 2026-10-25T22:00Z. It does not depend on the clock. Only `generatedAt`
does.

## Seed users

Every user has a `"user"`, an `"account"` (real Better Auth hash), a
`usernames`, and an `api_keys` row. `usernames.user_id` is the only foreign key
to `"user"`. `api_keys` and `user_settings` have no foreign key.

| User  | API key           | Area | Time zone        | Night  | VAT  | Other terms            |
| ----- | ----------------- | ---- | ---------------- | ------ | ---- | ---------------------- |
| alice | `sp_` + `a1` × 32 | FI   | Europe/Helsinki  | 22 → 7 | 25.5 | the service defaults   |
| bob   | `sp_` + `b2` × 32 | SE3  | Europe/Stockholm | 0 → 6  | 25   | margin 0               |
| carol | `sp_` + `c3` × 32 | GER  | Europe/Berlin    | 5 → 5  | 19   | start equals end       |
| dave  | `sp_` + `d4` × 32 | EE   | Europe/Helsinki  | 23 → 0 | 24   |                        |
| eve   | `sp_` + `e5` × 32 | FI   | Europe/Stockholm | 22 → 6 | 0    | all terms 0            |
| frank | `sp_` + `f6` × 32 | NO1  | Europe/Oslo      | 22 → 7 | 25   | no prices              |
| grace | `sp_` + `07` × 32 | —    | —                | —      | —    | no `user_settings` row |

Passwords are in `scripts/golden/http/seed.ts`. The seed also has four Better
Auth `"session"` rows with fixed tokens. `auth-vectors.json → cookieVectors`
gives their signed cookie values. The cookie value is
`encodeURIComponent(token + "." + base64(HMAC-SHA256(secret, token)))`.

## Notable TypeScript behaviour

- Expired sessions are accepted. `src/db.ts` makes `pg` return `TIMESTAMPTZ`
  as a string. Better Auth compares `session.expiresAt < new Date()`, and a
  string never compares as smaller. The session refresh (`updateAge`) never
  runs for the same reason. See `session-seeded-cookies` steps 2 and 13. This
  was also confirmed with the real clock outside the generator (procedure
  below).
- Responses that Better Auth builds (sign-in, sign-up, sign-out, and Better
  Auth errors) have no `ratelimit-*` headers, even when the login or global
  limiter counted the request. Responses that the service builds have them.
- On the Bearer routes the per-key limiter (60) overwrites the global headers
  (120). A 401 from the key check shows the global headers. A global 429 on a
  Bearer route never reaches the key check.
- `POST /health` is a 404 without rate-limit headers: the global limiter skips
  the path `/health` for every method.
- The global limit key is `x-real-ip`, else the first `x-forwarded-for` item
  (trimmed), else `unknown`. An empty `x-real-ip` gives the key `""`, not
  `unknown`.
- The rate-limit window is fixed and starts at the first request of a key.
  `RateLimit-Reset` and `Retry-After` are whole seconds, rounded up.
- `Authorization` matches `^Bearer\s+(\S+)$` without case. `bearer`, `BEARER`,
  extra spaces, and a tab work.
- `/api/v1/price/*` and `/api/v1/me/*` run the auth middleware for every
  method and for unknown paths. An unauthenticated unknown path is 401; an
  authenticated one is 404.
- A Hono 404 is `404 Not Found` with `text/plain; charset=UTF-8`. Malformed
  JSON on `PUT /api/v1/me/settings` is `Malformed JSON in request body` with
  `text/plain;charset=UTF-8` (no space).
- `PUT /api/v1/me/settings` validates the body only when the content type is
  JSON (`application/json`, `application/json; charset=utf-8`,
  `application/vnd.api+json`). With no content type, `text/plain`, form data,
  or `APPLICATION/JSON`, the body is ignored and the response is 200 with no
  change.
- A repeated query key (`duration=60&duration=120`, `area=FI&area=SE3`) fails
  validation.
- A `+` in a query string decodes to a space. The cheapest route turns spaces
  back into `+`, so `startTime=...+03:00` works without encoding.
- History validation runs the refinements even when a date is invalid:
  `from=abc&to=def` also reports `to: date range must not exceed 31 days`.
- `login-or-signup` with a numeric `username` is a 500 with the text
  `Internal Server Error`.
- `JSON.stringify(-0)` is `0`, so a stored `-0` price shows as `0`.
- The forecast `area` comes from the query (default `FI`), not from the user
  settings.
- Sign-in bodies have `"redirect":false`; sign-up bodies do not.
- A password that is equal after NFKC normalisation signs in. The length
  limits (8 .. 128) count UTF-16 code units before normalisation.

### Reproduce the expired-session result with the real clock

1. Create an empty database on the test server and apply `src/migrate.ts`.
2. Insert a user and a session that expired one day ago:
   `INSERT INTO "user" VALUES ('u1','n','n@x.y',false,NULL,now(),now());`
   `INSERT INTO "session" VALUES ('s1', now() - interval '1 day', 'tok1', now() - interval '8 days', now() - interval '8 days', NULL, NULL, 'u1');`
3. In a script run with `node --import tsx`, import `src/db.ts` first, then
   build `createAuth(pool)` with `BETTER_AUTH_SECRET` set.
4. Call `auth.api.getSession` with the header
   `cookie: better-auth.session_token=<encodeURIComponent("tok1." + base64(HMAC-SHA256(secret, "tok1")))>`.
5. Expected result (observed on 2026-10-14): the call returns the session, and
   `typeof session.session.expiresAt` is `"string"`. Without the
   `src/db.ts` import, `pg` returns a `Date` and Better Auth rejects the
   session (also observed).

## Not covered

- Zero-seeded forecast quarters (reason `Some quarters had no input data and
were zero-seeded`). The model fills missing inputs from any Fingrid row, so
  no seed found through HTTP reached this state. The forecast domain golden
  files cover `buildPredictedSeries`.
- A real socket. Transport headers, HTTP/1.1 parsing, and the client address
  of `@hono/node-server` are not recorded.
- The startup jobs and the scheduler. They call upstream APIs.
- Better Auth's own `/api/auth/*` handler. The service does not mount it.
- Concurrent requests.
