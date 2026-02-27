# Research Dossier: OpenAPI Schema as Single Source of Truth

Date: 2026-02-27
Author: Research Agent
Status: **Draft** — awaiting owner review

---

## Problem Statement

API documentation exists in three independent locations with no enforced link between them:

1. **`README.md`** — endpoint table, query params, full response JSON examples, error codes, rate limits (~80 lines)
2. **`src/ui.ts`** — curl examples (dynamic) + response type `<details>` blocks (static) + error codes (static) (~90 lines)
3. **`src/app.ts`** — actual route handlers with inline type annotations and manual validation (~200 lines of validation)

These will inevitably drift apart. Adding a field to a response requires updating three files. Manual request validation in `app.ts` (e.g., `if (vatPercent < 0)`) duplicates what Zod schemas would enforce declaratively.

### Goal

Define request and response types **once** in Zod schemas that simultaneously:

1. Validate requests at runtime (replace manual `if` checks)
2. Validate response shapes during development
3. Generate the OpenAPI spec automatically (replace the stub at `/api/v1/openapi.json`)
4. Serve interactive API documentation (replace static HTML docs in UI)
5. Provide TypeScript types (replace inline type annotations)

---

## Target Users

| User                               | Job-to-be-done                                          |
| ---------------------------------- | ------------------------------------------------------- |
| API consumer (Home Assistant user) | Read accurate, interactive API docs; try endpoints live |
| Developer (maintainer)             | Define types once; trust that docs match implementation |
| AI agent                           | Use OpenAPI spec for code generation and validation     |

---

## Topic 1: @hono/zod-openapi

### Verified — npm and documentation reviewed (2026-02-27)

| Attribute             | Detail                                                |
| --------------------- | ----------------------------------------------------- |
| **Package**           | `@hono/zod-openapi` v1.2.2                            |
| **Weekly downloads**  | 404,858                                               |
| **License**           | MIT                                                   |
| **Peer dependencies** | `hono >= 4.3.6`, `zod ^4.0.0`                         |
| **Our Hono version**  | 4.12.2 (satisfies ✓)                                  |
| **Our Zod version**   | 4.3.6 (transitive via better-auth, satisfies ✓)       |
| **Unpacked size**     | 150 kB                                                |
| **Total files**       | 10                                                    |
| **Repository**        | github.com/honojs/middleware (official Hono monorepo) |
| **Maintainer**        | Yusuke Wada (Hono creator)                            |

### How it works

`OpenAPIHono` extends `Hono` — all existing middleware, route patterns, and context variables work unchanged. Routes are defined with `createRoute()` which accepts Zod schemas for:

- **`request.params`** — path parameters (e.g., `{id}`)
- **`request.query`** — query string parameters (e.g., `?duration=180`)
- **`request.body`** — JSON request body (e.g., PUT settings payload)
- **`request.headers`** — request headers (note: keys must be lowercase)
- **`responses`** — response bodies per status code, each with a Zod schema

The handler receives validated, typed data via `c.req.valid('param')`, `c.req.valid('query')`, `c.req.valid('json')`.

### Key features

- **`app.doc('/path', config)`** — auto-generates OpenAPI 3.0 spec from all registered routes
- **`app.doc31('/path', config)`** — OpenAPI 3.1 support
- **`defaultHook`** — global validation error handler (DRY error formatting)
- **`route.middleware`** — per-route middleware in route definition
- **`.openapi('Name')`** — registers schemas as `#/components/schemas/Name` refs
- **`route.hide`** — exclude specific routes from the spec
- **`app.openAPIRegistry`** — access the registry for custom components (security schemes, etc.)

### Compatibility with our codebase

| Concern                                               | Assessment                                         |
| ----------------------------------------------------- | -------------------------------------------------- |
| Existing `Hono` middleware (logger, CORS, rate limit) | Works — `OpenAPIHono` extends `Hono`               |
| `app.use()` patterns for auth                         | Works — `app.use(path, middleware)` unchanged      |
| Context variables (`c.get("db")`, `c.get("userId")`)  | Works — same `Variables` env type                  |
| `@hono/node-server`                                   | Works — `OpenAPIHono` produces a standard Hono app |
| `hono-rate-limiter`                                   | Works — operates on the same `Context`             |

### Migration approach

Routes can be migrated incrementally — `OpenAPIHono` supports both `app.get()` (plain Hono) and `app.openapi(route, handler)` (OpenAPI) on the same instance. Non-API routes (health, auth, HTML) can remain as plain routes.

**Evidence**: npm page shows v1.2.2, 404k weekly downloads, 10 files, MIT license. Hono examples page documents Zod OpenAPI as a first-class integration. README confirms OpenAPIHono extends Hono with full middleware compatibility. [Source: npmjs.com/package/@hono/zod-openapi, hono.dev/examples/zod-openapi]

---

## Topic 2: @scalar/hono-api-reference

### Verified — npm reviewed (2026-02-27)

| Attribute            | Detail                               |
| -------------------- | ------------------------------------ |
| **Package**          | `@scalar/hono-api-reference` v0.9.46 |
| **Weekly downloads** | 184,386                              |
| **License**          | MIT                                  |
| **Dependencies**     | 1                                    |
| **Unpacked size**    | 13.6 kB                              |
| **Total files**      | 17                                   |

### What it does

Provides a single Hono middleware that serves a beautiful, interactive API documentation page from an OpenAPI spec URL. Features:

- Interactive "Try it" functionality — users can test endpoints live from the docs page
- Dark mode support (matches our UI aesthetic)
- Renders request/response schemas, examples, authentication requirements
- Zero configuration beyond the spec URL

### Usage

```typescript
import { apiReference } from "@scalar/hono-api-reference";

app.get(
  "/api/docs",
  apiReference({
    spec: { url: "/api/v1/openapi.json" },
    theme: "dark",
  }),
);
```

**Evidence**: npm page shows v0.9.46, 184k weekly downloads, MIT license, 13.6 kB. Hono examples page documents Scalar as a supported OpenAPI viewer. [Source: npmjs.com/package/@scalar/hono-api-reference, hono.dev/examples/scalar]

---

## Topic 3: Zod as a Direct Dependency

Zod 4.3.6 is already in `node_modules` as a transitive dependency via `better-auth`. However, `@hono/zod-openapi` requires `zod ^4.0.0` as a peer dependency. For stability and explicitness, Zod should be added as a direct dependency.

The `z` object must be imported from `@hono/zod-openapi` (not `zod` directly) to get the `.openapi()` extension method on schemas.

---

## Topic 4: Current Manual Validation to Replace

| Route                               | Current manual validation                                                         | Lines | Zod replacement                                                                                                      |
| ----------------------------------- | --------------------------------------------------------------------------------- | ----- | -------------------------------------------------------------------------------------------------------------------- |
| `PUT /api/v1/me/settings`           | Type assertion + 5 `if` checks (vatPercent, nightStart, nightEnd, area, timezone) | ~35   | `z.object({ vatPercent: z.number().min(0).max(100), ... }).partial()`                                                |
| `GET /api/v1/price/cheapest`        | `parseDuration()` + 2 ISO 8601 checks                                             | ~28   | `z.object({ duration: z.coerce.number().int().min(1).max(1440), startTime: z.string().datetime().optional(), ... })` |
| `GET /api/public/spot`              | `area?.toUpperCase()` + `isValidAreaCode()`                                       | ~5    | `z.object({ area: z.enum([...]).optional().default("FI") })`                                                         |
| `POST /api/session/login-or-signup` | `validateUsername()` + `!password`                                                | ~8    | `z.object({ username: z.string().regex(...), password: z.string().min(8).max(128) })`                                |

**Total manual validation to replace: ~76 lines → declarative Zod schemas**

---

## Topic 5: Routes to Migrate

### Must migrate (API endpoints with request/response contracts)

| Route                        | Request schema                                       | Response schema                          |
| ---------------------------- | ---------------------------------------------------- | ---------------------------------------- |
| `GET /api/v1/price/now`      | None (auth only)                                     | `TotalPriceSchema`                       |
| `GET /api/v1/price/today`    | None                                                 | `PriceListSchema`                        |
| `GET /api/v1/price/tomorrow` | None                                                 | `PriceListSchema` or `UnavailableSchema` |
| `GET /api/v1/price/cheapest` | `CheapestQuerySchema` (duration, startTime, endTime) | `CheapestWindowSchema`                   |
| `GET /api/public/spot`       | `SpotQuerySchema` (area)                             | `PublicSpotSchema`                       |
| `PUT /api/v1/me/settings`    | `UserSettingsSchema.partial()`                       | `UserSettingsSchema`                     |
| `GET /api/v1/me/settings`    | None                                                 | `UserSettingsSchema`                     |
| `GET /api/v1/me/chart`       | None                                                 | `ChartDataSchema`                        |

### Keep as plain Hono routes (no OpenAPI benefit)

| Route                               | Reason                                                              |
| ----------------------------------- | ------------------------------------------------------------------- |
| `GET /health`                       | Infrastructure, not API                                             |
| `GET /`                             | HTML page                                                           |
| `POST /api/session/login-or-signup` | Better Auth wraps the response; complex flow not suited for OpenAPI |
| `POST /api/session/sign-out`        | Better Auth response                                                |
| `GET /api/session`                  | Better Auth response                                                |
| `GET /api/keys`                     | Session-only, internal UI endpoint                                  |
| `POST /api/keys/regenerate`         | Session-only, internal UI endpoint                                  |

### Note on login-or-signup

The login/signup route delegates to Better Auth (`auth.api.signInEmail`, `auth.api.signUpEmail`) which returns its own Response objects. Wrapping this in OpenAPI would require intercepting Better Auth's response to validate against our schema, which adds complexity without benefit. The request validation (username/password) can still use Zod independently.

---

## Topic 6: Shared Schema Module Design

```
src/
  api-schemas.ts          # All Zod schemas + OpenAPI metadata
  routes/
    price-now.ts          # Route definition + handler
    price-today.ts
    price-tomorrow.ts
    price-cheapest.ts
    public-spot.ts
    settings.ts
    chart.ts
  app.ts                  # OpenAPIHono setup, middleware, non-API routes
```

### Shared schemas (defined once, reused everywhere)

```typescript
// src/api-schemas.ts
import { z } from "@hono/zod-openapi";

export const TotalPriceSchema = z
  .object({
    deliveryStart: z.string().openapi({ example: "2026-02-26T10:00:00Z" }),
    deliveryEnd: z.string().openapi({ example: "2026-02-26T10:15:00Z" }),
    localStart: z.string().openapi({ example: "2026-02-26T12:00:00+02:00" }),
    localEnd: z.string().openapi({ example: "2026-02-26T12:15:00+02:00" }),
    spotCentsKwh: z.number().openapi({ example: 5.23 }),
    marginCentsKwh: z.number().openapi({ example: 0.5 }),
    transferCentsKwh: z.number().openapi({ example: 2.5 }),
    taxCentsKwh: z.number().openapi({ example: 2.794 }),
    vatCentsKwh: z.number().openapi({ example: 2.756 }),
    totalCentsKwh: z.number().openapi({ example: 13.78 }),
    isNightRate: z.boolean().openapi({ example: false }),
  })
  .openapi("TotalPrice");

export const ErrorSchema = z
  .object({
    error: z.string().openapi({ example: "Description of what went wrong" }),
  })
  .openapi("Error");

// ... PriceListSchema, CheapestWindowSchema, etc.
```

---

## Topic 7: UI Changes

### Remove from `src/ui.ts`

The static `<details>` response type blocks (~90 lines of hardcoded HTML showing JSON response shapes). These are the documentation that will drift.

### Replace with

A link to the Scalar interactive docs page served at `/api/docs`. The API panel keeps:

- The API key display + copy button
- The dynamic curl examples (already generated from `state.apiKey`)
- A prominent link: "Full API reference → /api/docs"

The Scalar docs page provides:

- All endpoint descriptions with request/response schemas
- Live examples from the OpenAPI spec
- "Try it" functionality with the user's auth
- Always in sync with the actual routes

---

## Topic 8: README Changes

Replace the static API documentation section (~80 lines of JSON examples) with:

1. The endpoint summary table (keep — it's a quick reference)
2. A link to the interactive docs: "See [Interactive API Documentation](/api/docs) for full request/response schemas and live examples"
3. Remove the static JSON response examples (they're now auto-generated)

Keep the query parameters tables and error codes table (these are concise reference and less prone to drift).

---

## Alternatives Considered

### A: Keep current approach, add tests to detect drift

- Write tests that parse README and UI HTML to verify field names match actual response shapes
- Pro: No dependency changes
- Con: Fragile tests, still manual sync, no runtime validation

### B: Remove docs from UI only, keep README as source of truth

- Delete static docs from `ui.ts`, keep README manually updated
- Pro: Simplest change
- Con: README still drifts from implementation; no runtime validation

### C: OpenAPI schema as single source of truth (recommended)

- Zod schemas define types once → validation + docs + types
- Pro: Single source of truth, runtime validation, interactive docs
- Con: Route refactor, 2 new dependencies

### Comparison

| Criterion                  | A: Test for drift | B: README only   | C: OpenAPI (recommended) |
| -------------------------- | ----------------- | ---------------- | ------------------------ |
| Single source of truth     | No                | Partial          | **Yes**                  |
| Runtime request validation | No                | No               | **Yes**                  |
| Interactive docs           | No                | No               | **Yes**                  |
| New dependencies           | 0                 | 0                | 2 (+ zod as direct)      |
| Implementation effort      | ~2h               | ~30min           | **~4h**                  |
| Ongoing maintenance burden | High (3 files)    | Medium (2 files) | **Low (1 schema)**       |

---

## Assumptions

| #   | Assumption                                                   | Risk if Wrong                                           |
| --- | ------------------------------------------------------------ | ------------------------------------------------------- |
| A1  | `@hono/zod-openapi` works with our existing middleware stack | May need `$()` type utility for middleware chains       |
| A2  | Zod 4.3.6 satisfies peer dependency `^4.0.0`                 | Already verified: 4.3.6 ∈ ^4.0.0 ✓                      |
| A3  | Scalar renders well in dark mode                             | Has theme support; may need CSS tweaks                  |
| A4  | Better Auth routes can remain as plain Hono routes           | Confirmed: OpenAPIHono supports both patterns           |
| A5  | Incremental migration is possible                            | Confirmed: plain routes and OpenAPI routes coexist      |
| A6  | Build size impact is acceptable                              | @hono/zod-openapi is 150 kB unpacked; Scalar is 13.6 kB |

## Unknowns

| #   | Unknown                                                                        | Impact                                     | Mitigation                               |
| --- | ------------------------------------------------------------------------------ | ------------------------------------------ | ---------------------------------------- |
| U1  | `@hono/zod-openapi` behavior with `c.get("db")` context variables              | Route handlers might need type adjustments | Test first route before migrating all    |
| U2  | Scalar dark mode appearance with our color scheme                              | May look out of place                      | Scalar has `theme` config; can customize |
| U3  | Impact on test mocking (routes are now `app.openapi()` instead of `app.get()`) | Tests may need minor updates               | Request/response interface unchanged     |
| U4  | `tsup` bundle size increase with Zod + OpenAPI schemas                         | May increase bundle by ~100-200 KB         | Acceptable for server-side bundle        |

---

## Evidence Sufficiency Assessment

**Confidence: HIGH for proceeding to planning.**

- `@hono/zod-openapi` is the official Hono OpenAPI integration, maintained by Hono's creator (404k downloads/week)
- `@scalar/hono-api-reference` is the recommended OpenAPI viewer for Hono (184k downloads/week)
- Both packages are MIT licensed with active maintenance
- Peer dependency compatibility verified: Hono 4.12.2 ✓, Zod 4.3.6 ✓
- `OpenAPIHono` extends `Hono` — confirmed backward compatible with existing middleware
- Incremental migration confirmed — plain routes and OpenAPI routes coexist
- 8 routes to migrate, 7 routes to keep as-is — scope is well-defined

---

## Summary of Recommendations

| Decision            | Choice                                                        | Confidence |
| ------------------- | ------------------------------------------------------------- | ---------- |
| Schema library      | `@hono/zod-openapi` v1.2.2 (official Hono integration)        | High       |
| Interactive docs    | `@scalar/hono-api-reference` v0.9.46                          | High       |
| Zod                 | Add as direct dependency (already transitive via better-auth) | High       |
| Migration scope     | 8 API routes (price/\*, public/spot, me/settings, me/chart)   | High       |
| Non-migrated routes | Health, HTML, auth, keys — keep as plain Hono                 | High       |
| UI changes          | Remove static response docs, link to /api/docs                | High       |
| README changes      | Keep endpoint table, replace JSON examples with docs link     | High       |
| Estimated effort    | ~4 hours                                                      | Medium     |
