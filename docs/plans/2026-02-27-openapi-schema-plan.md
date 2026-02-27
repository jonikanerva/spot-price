# Implementation Plan: OpenAPI Schema as Single Source of Truth

Date: 2026-02-27
Author: RPI Orchestrator
Status: Draft — awaiting owner review
Research basis: `docs/research/2026-02-27-openapi-schema-research.md` (Approved 2026-02-27)

---

## Goal

Eliminate API documentation duplication by defining request and response types once in Zod schemas that simultaneously validate requests at runtime, generate the OpenAPI spec, serve interactive docs, and provide TypeScript types. As a prerequisite, upgrade all dependencies to latest versions and enforce Node.js 24 LTS.

## Scope Boundaries

**In scope:**

- Enforce Node.js 24 LTS (`engines`, `.node-version`)
- Upgrade all dependencies to latest (`npx npm-check-updates -u`) and fix breakage
- Install `@hono/zod-openapi`, `@scalar/hono-api-reference`, `zod` (as direct dep)
- Create shared Zod schemas in `src/api-schemas.ts` for all API request/response contracts
- Migrate 8 API routes from plain Hono to `OpenAPIHono` + `createRoute()`
- Auto-generate OpenAPI 3.1 spec at `/api/v1/openapi.json`
- Serve interactive Scalar docs at `/api/docs`
- Remove static response type docs from `src/ui.ts`, link to `/api/docs`
- Replace ~76 lines of manual validation in `src/app.ts` with Zod schema validation
- Update `README.md` — keep endpoint table, replace JSON examples with docs link
- Update all tests for new route structure

**Out of scope:**

- Migrating auth routes (login-or-signup, sign-out, session) to OpenAPI
- Migrating key management routes to OpenAPI
- Migrating health or HTML routes to OpenAPI
- Custom Scalar theme/CSS
- Code generation from OpenAPI spec

---

## Tasks

### T0: Upgrade Node.js enforcement and all dependencies

Upgrade all dependencies to latest versions, enforce Node.js 24 LTS, and fix any issues arising from major version bumps (ESLint 9→10, Vitest 3→4, @types/node 22→25).

**Steps:**

1. Update `.node-version` to latest Node 24 LTS patch if needed
2. Update `engines` in `package.json` from `>=22.0.0` to `>=24.0.0`
3. Run `npx npm-check-updates -u` to update all dependency ranges
4. Run `npm install` to install updated packages
5. Fix ESLint 10 breaking changes (flat config API changes, rule renames)
6. Fix Vitest 4 breaking changes (API changes, config updates)
7. Fix @types/node 25 type incompatibilities if any
8. Fix any other type or runtime breakage from upgrades
9. Run full verification: typecheck, lint, format, test, e2e, build

| Detail                                            | Acceptance                                         |
| ------------------------------------------------- | -------------------------------------------------- |
| `.node-version` and `engines` reflect Node 24 LTS | `node --version` matches, `engines: >=24.0.0`      |
| All deps at latest versions                       | `npx npm-check-updates` shows no updates           |
| No regressions from upgrades                      | All checks pass: typecheck, lint, test, e2e, build |

**Known major version bumps to address:**

| Package             | From  | To    | Risk   | Notes                                |
| ------------------- | ----- | ----- | ------ | ------------------------------------ |
| `eslint`            | ^9    | ^10   | Medium | Flat config may need adjustments     |
| `@eslint/js`        | ^9    | ^10   | Medium | Must match eslint major version      |
| `vitest`            | ^3    | ^4    | Medium | API changes, possible config updates |
| `@types/node`       | ^22   | ^25   | Low    | Type definition changes              |
| `typescript`        | ^5.7  | ^5.9  | Low    | Minor version, backward compatible   |
| `typescript-eslint` | ^8.24 | ^8.56 | Low    | Minor version, backward compatible   |

**Effort:** 30–60 minutes (depends on breakage severity)

---

### T1: Install OpenAPI dependencies

```bash
npm install @hono/zod-openapi zod
npm install -D @scalar/hono-api-reference
```

Note: `zod` is already a transitive dep via better-auth, but adding as direct dep for stability and explicit peer dependency satisfaction.

| Detail                                  | Acceptance                                          |
| --------------------------------------- | --------------------------------------------------- |
| `@hono/zod-openapi` in dependencies     | `npm ls @hono/zod-openapi` shows installed          |
| `zod` in dependencies                   | `npm ls zod` shows direct dep                       |
| `@scalar/hono-api-reference` in devDeps | `npm ls @scalar/hono-api-reference` shows installed |
| All checks still pass after install     | typecheck, lint, test green                         |

**Effort:** 5 minutes

---

### T2: Create shared Zod schemas (`src/api-schemas.ts`)

Define all API request and response schemas in one module. Import `z` from `@hono/zod-openapi` to get `.openapi()` extension.

**Response schemas:**

| Schema                 | Used by                           | Fields                                                                                                                                                 |
| ---------------------- | --------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `TotalPriceSchema`     | `/price/now`                      | deliveryStart, deliveryEnd, localStart, localEnd, spotCentsKwh, marginCentsKwh, transferCentsKwh, taxCentsKwh, vatCentsKwh, totalCentsKwh, isNightRate |
| `PriceListSchema`      | `/price/today`, `/price/tomorrow` | `{ prices: TotalPriceSchema[], resolutionMinutes: number }`                                                                                            |
| `CheapestWindowSchema` | `/price/cheapest`                 | start, end, localStart, localEnd, averageTotalCentsKwh, durationMinutes, prices[]                                                                      |
| `PublicSpotSchema`     | `/public/spot`                    | `{ prices: SpotPriceEntry[], resolutionMinutes: number }`                                                                                              |
| `UserSettingsSchema`   | `/me/settings` GET and PUT        | marginCentsKwh, transferDayCentsKwh, transferNightCentsKwh, taxCentsKwh, vatPercent, nightStartHour, nightEndHour, timezone, area                      |
| `ChartDataSchema`      | `/me/chart`                       | `{ today: PriceEntry[], tomorrow: PriceEntry[] }`                                                                                                      |
| `ErrorSchema`          | All error responses               | `{ error: string }`                                                                                                                                    |
| `UnavailableSchema`    | `/price/tomorrow` (no data)       | `{ available: false, message: string }`                                                                                                                |

**Request schemas:**

| Schema                     | Used by            | Fields                                                        |
| -------------------------- | ------------------ | ------------------------------------------------------------- |
| `CheapestQuerySchema`      | `/price/cheapest`  | duration (int, 1–1440), startTime? (ISO), endTime? (ISO)      |
| `SpotQuerySchema`          | `/public/spot`     | area? (enum of 21 codes, default "FI")                        |
| `UserSettingsUpdateSchema` | `/me/settings` PUT | All UserSettings fields as `.partial()` with validation rules |

| Detail                                              | Acceptance               |
| --------------------------------------------------- | ------------------------ |
| All schemas export typed Zod objects                | TypeScript compiles      |
| Each schema has `.openapi("Name")` registration     | Schemas appear in spec   |
| Request schemas include validation (min, max, enum) | Invalid input rejected   |
| Examples provided via `.openapi({ example: ... })`  | Spec has useful examples |

**Effort:** 45 minutes

---

### T3: Refactor `src/app.ts` to use `OpenAPIHono`

1. Replace `new Hono(...)` with `new OpenAPIHono(...)` (same middleware, same env types)
2. Migrate 8 routes from `app.get()` / `app.put()` to `app.openapi(createRoute(...), handler)`
3. Replace manual `if` validation with `c.req.valid('query')` / `c.req.valid('json')`
4. Add `defaultHook` for consistent validation error formatting (400 + JSON error)
5. Keep non-API routes (health, HTML, auth, keys) as plain Hono routes

**Routes to migrate:**

| Route                     | Method | Request validation         | Response schema                          |
| ------------------------- | ------ | -------------------------- | ---------------------------------------- |
| `/api/v1/price/now`       | GET    | None (auth only)           | `TotalPriceSchema` or `ErrorSchema`      |
| `/api/v1/price/today`     | GET    | None                       | `PriceListSchema` or `ErrorSchema`       |
| `/api/v1/price/tomorrow`  | GET    | None                       | `PriceListSchema` or `UnavailableSchema` |
| `/api/v1/price/cheapest`  | GET    | `CheapestQuerySchema`      | `CheapestWindowSchema` or `ErrorSchema`  |
| `/api/public/spot`        | GET    | `SpotQuerySchema`          | `PublicSpotSchema` or `ErrorSchema`      |
| `GET /api/v1/me/settings` | GET    | None (session auth)        | `UserSettingsSchema`                     |
| `PUT /api/v1/me/settings` | PUT    | `UserSettingsUpdateSchema` | `UserSettingsSchema` or `ErrorSchema`    |
| `GET /api/v1/me/chart`    | GET    | None (session auth)        | `ChartDataSchema`                        |

| Detail                                                    | Acceptance                                                          |
| --------------------------------------------------------- | ------------------------------------------------------------------- |
| `OpenAPIHono` replaces `Hono`                             | App starts, all middleware works                                    |
| Manual validation `if` blocks removed from route handlers | Grep confirms no manual `parseDuration`, `if (vatPercent < 0)` etc. |
| `defaultHook` returns `{ error: string }` on 400          | Consistent error format across all routes                           |
| Non-API routes unchanged                                  | Health, auth, keys, HTML all still work                             |

**Effort:** 60 minutes

---

### T4: Auto-generate OpenAPI spec and serve Scalar docs

1. Add `app.doc31('/api/v1/openapi.json', { ... })` for OpenAPI 3.1 spec
2. Register security scheme (Bearer API key) in the OpenAPI registry
3. Add Scalar middleware at `/api/docs` pointing to the spec URL
4. Configure Scalar with dark theme

```typescript
import { apiReference } from "@scalar/hono-api-reference";

app.doc31("/api/v1/openapi.json", {
  openapi: "3.1.0",
  info: { title: "Spot Price API", version: "1.0.0", description: "..." },
  servers: [{ url: "https://spot.calmdonut.com" }],
});

app.get(
  "/api/docs",
  apiReference({
    spec: { url: "/api/v1/openapi.json" },
    theme: "dark",
  }),
);
```

| Detail                                               | Acceptance                                             |
| ---------------------------------------------------- | ------------------------------------------------------ |
| `GET /api/v1/openapi.json` returns valid OpenAPI 3.1 | JSON parses, contains all 8 routes                     |
| `GET /api/docs` renders Scalar interactive docs      | Page loads, shows endpoint list                        |
| Security scheme documented                           | Spec shows Bearer auth requirement on protected routes |
| Scalar dark theme matches app aesthetic              | Visual inspection                                      |

**Effort:** 20 minutes

---

### T5: Update `src/ui.ts` — remove static docs, link to Scalar

1. Remove static `<details>` response type blocks from the API panel (~90 lines of hardcoded HTML)
2. Add a prominent link to `/api/docs` in the API panel: "Full API reference with interactive examples"
3. Keep the API key display, copy button, and curl examples (these are dynamic and useful)

| Detail                                         | Acceptance                               |
| ---------------------------------------------- | ---------------------------------------- |
| Static response type HTML removed from `ui.ts` | Grep confirms no `<details>` type blocks |
| Link to `/api/docs` added in API panel         | Visible in UI                            |
| API key + curl examples still present          | E2E test: API panel shows key            |

**Effort:** 15 minutes

---

### T6: Update `README.md`

1. Keep the endpoint summary table (quick reference)
2. Remove static JSON response examples (~80 lines)
3. Add link: "See [Interactive API Documentation](https://spot.calmdonut.com/api/docs) for full request/response schemas and live examples"
4. Keep query parameter tables and error codes table (concise, less prone to drift)

| Detail                              | Acceptance        |
| ----------------------------------- | ----------------- |
| JSON response examples removed      | Shorter README    |
| Link to interactive docs added      | Present in README |
| Endpoint table and error codes kept | Still present     |

**Effort:** 15 minutes

---

### T7: Update tests

1. Update unit tests in `src/app.test.ts` and `src/api-routes.test.ts` if route registration changed
2. Verify E2E tests pass (route behavior unchanged, only registration method changed)
3. Add test: `GET /api/v1/openapi.json` returns valid JSON with expected route count
4. Add test: validation error returns 400 with `{ error: string }` format
5. Update any mocks that reference old route patterns

| Detail                             | Acceptance               |
| ---------------------------------- | ------------------------ |
| All existing unit tests pass       | `npm test` green         |
| All E2E tests pass                 | `npm run test:e2e` green |
| New test for OpenAPI spec endpoint | Test exists and passes   |
| Validation error format test       | Test exists and passes   |

**Effort:** 30 minutes

---

### T8: Verification pass

| Check      | Command                | Expected                  |
| ---------- | ---------------------- | ------------------------- |
| TypeScript | `npm run typecheck`    | Pass                      |
| Lint       | `npm run lint`         | Pass                      |
| Format     | `npm run format:check` | Pass                      |
| Unit tests | `npm test`             | Pass (all existing + new) |
| E2E tests  | `npm run test:e2e`     | Pass (all 11+)            |
| Build      | `npm run build`        | Pass                      |

**Effort:** 10 minutes

---

## Task Order

```
T0 (deps upgrade + Node 24 enforcement)
  → T1 (install OpenAPI deps)
    → T2 (Zod schemas)
      → T3 (refactor app.ts to OpenAPIHono)
        → T4 (spec + Scalar docs)
        → T5 (update ui.ts)
        → T6 (update README)
      → T7 (update tests)
    → T8 (verification)
```

T0 must complete first — all subsequent work depends on a clean, upgraded baseline. T1–T2 are sequential foundations. T3 is the main refactor. T4–T6 can proceed after T3. T7 runs throughout. T8 finalizes.

**Total estimated effort: ~4–5 hours** (T0 adds ~30–60 min to the original ~4h estimate)

---

## Risks and Mitigations

| #   | Risk                                                             | Likelihood | Impact | Mitigation                                                         |
| --- | ---------------------------------------------------------------- | :--------: | :----: | ------------------------------------------------------------------ |
| R0  | ESLint 10 or Vitest 4 major changes break config or tests        |   Medium   | Medium | Fix incrementally; can pin to current major if breakage is severe  |
| R1  | `@hono/zod-openapi` doesn't work with existing middleware chain  |    Low     | Medium | OpenAPIHono extends Hono — confirmed compatible; test T1 first     |
| R2  | Zod schema types don't match current response shapes exactly     |   Medium   |  Low   | Write schemas from actual response types; test with existing tests |
| R3  | Scalar dark mode clashes with app aesthetic                      |    Low     |  Low   | Scalar has theme config; can customize                             |
| R4  | `defaultHook` validation errors differ from current error format |   Medium   |  Low   | Match existing `{ error: string }` format in hook                  |
| R5  | `@types/node` 25 introduces incompatible type changes            |    Low     | Medium | Fix type errors; can pin to @types/node@24 if severe               |

---

## Acceptance Criteria

| #    | Criterion                                                               | Verification                       |
| ---- | ----------------------------------------------------------------------- | ---------------------------------- |
| AC0  | Node.js 24 LTS enforced in `.node-version` and `engines`                | File contents verified             |
| AC1  | All dependencies at latest versions with no regressions                 | `npx npm-check-updates` shows none |
| AC2  | `GET /api/v1/openapi.json` returns valid OpenAPI 3.1 spec               | Unit test                          |
| AC3  | Spec contains all 8 migrated routes with request/response schemas       | Inspect spec JSON                  |
| AC4  | `GET /api/docs` renders Scalar interactive documentation                | Manual / E2E test                  |
| AC5  | Manual validation removed from route handlers                           | Code review: no inline `if` checks |
| AC6  | Invalid request returns 400 with `{ error: string }` via Zod validation | Unit test                          |
| AC7  | Static response type docs removed from `ui.ts`                          | Grep for `<details>` blocks        |
| AC8  | README JSON examples replaced with link to interactive docs             | File inspection                    |
| AC9  | All existing unit tests pass                                            | `npm test` green                   |
| AC10 | All E2E tests pass                                                      | `npm run test:e2e` green           |
| AC11 | Build succeeds                                                          | `npm run build` green              |

---

## Quality Gate Checklist (Pre-Merge)

- [ ] All acceptance criteria (AC0–AC11) verified
- [ ] `npm run typecheck` passes
- [ ] `npm run lint` passes
- [ ] `npm run format:check` passes
- [ ] `npm test` passes (all tests)
- [ ] `npm run test:e2e` passes (all tests)
- [ ] `npm run build` passes
- [ ] No `any` types introduced
- [ ] No code duplication
- [ ] Net code reduction confirmed (manual validation + static docs removed)
- [ ] All dependencies at latest versions
