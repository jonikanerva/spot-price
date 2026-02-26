# Implementation Plan: Multi-Area Nord Pool Spot Price Support

Date: 2026-02-26
Author: RPI Orchestrator
Status: Draft — awaiting owner review
Research basis: `docs/research/2026-02-26-multi-area-support-research.md` (Approved 2026-02-26)

---

## Goal

Allow users to select their Nord Pool delivery area and timezone from settings. All 21 supported areas are fetched daily. API responses and chart timestamps reflect the user's configured area and timezone. Defaults remain FI / Europe/Helsinki.

## Scope Boundaries

**In scope:**

- Database migration: add `area` column to `user_settings`
- Area/timezone constants module with mapping and validation
- Ingestion: fetch all 21 areas in a single API call per date
- Replace hardcoded `AREA = "FI"` in `app.ts` with user's area from settings
- Public endpoint `/api/public/spot`: add optional `?area=` query param (default `FI`)
- Settings UI: area dropdown + timezone dropdown (13 Nord Pool-relevant timezones only)
- Settings API: accept `area` in PUT payload, validate against allowed list
- Auto-set timezone when area changes in UI
- Update existing tests, add tests for area validation and multi-area ingestion

**Out of scope:**

- Country-specific default tax/VAT/transfer rates (users configure manually)
- Per-area localized labels or translations
- Area comparison charts (showing multiple areas side-by-side)
- Filtering which areas to fetch based on active users

---

## Tasks

### T1: Area/timezone constants module

Create `src/areas.ts` with typed constants for all 21 areas, their display names, and timezone mappings.

```typescript
// src/areas.ts
interface DeliveryArea {
  readonly code: string;
  readonly name: string;
  readonly country: string;
  readonly timezone: string;
}

const DELIVERY_AREAS: readonly DeliveryArea[] = [
  { code: "FI", name: "Finland", country: "FI", timezone: "Europe/Helsinki" },
  {
    code: "SE1",
    name: "Sweden — Luleå",
    country: "SE",
    timezone: "Europe/Stockholm",
  },
  // ... all 21
];

const VALID_AREA_CODES: ReadonlySet<string>;
const SUPPORTED_TIMEZONES: readonly string[];
const getDefaultTimezone: (areaCode: string) => string;
const isValidAreaCode: (code: string) => boolean;
const isValidTimezone: (tz: string) => boolean;
```

| Detail                                                   | Acceptance                     |
| -------------------------------------------------------- | ------------------------------ |
| All 21 areas with codes, names, countries, timezones     | Module exports typed constants |
| Validation functions for area codes and timezones        | Unit tests pass                |
| `getDefaultTimezone("SE3")` returns `"Europe/Stockholm"` | Unit test                      |

**Effort:** 20 minutes

---

### T2: Database migration — add `area` column to `user_settings`

Create `src/migrations/008_add_area_to_user_settings.sql`:

```sql
ALTER TABLE user_settings ADD COLUMN area TEXT NOT NULL DEFAULT 'FI';
```

| Detail                                         | Acceptance                |
| ---------------------------------------------- | ------------------------- |
| Existing users get `area = 'FI'` automatically | Migration runs on startup |
| New users also get `area = 'FI'` (DEFAULT)     | Verified with test DB     |

**Effort:** 10 minutes

---

### T3: Update types and user-settings module

- Add `area: string` to `UserSettings` interface in `types.ts`
- Update `UserSettingsRow` in `user-settings.ts` to include `area`
- Update `rowToSettings` mapping
- Update `upsertUserSettings` to include `area` column
- Update `DEFAULT_SETTINGS` to include `area: "FI"`

| Detail                                   | Acceptance          |
| ---------------------------------------- | ------------------- |
| `UserSettings.area` field exists         | TypeScript compiles |
| `upsertUserSettings` persists `area`     | Unit test           |
| `DEFAULT_SETTINGS` includes `area: "FI"` | Verified            |

**Effort:** 15 minutes

---

### T4: Settings API — accept and validate `area`

Update `PUT /api/v1/me/settings` in `app.ts`:

- Accept `area` in request payload
- Validate against `isValidAreaCode()` from `areas.ts`
- Validate timezone against `isValidTimezone()` from `areas.ts`
- Return 400 if invalid area or timezone

| Detail                                                 | Acceptance          |
| ------------------------------------------------------ | ------------------- |
| `PUT` with `area: "SE3"` succeeds and persists         | API test            |
| `PUT` with `area: "INVALID"` returns 400               | API test            |
| `PUT` with `timezone: "US/Pacific"` returns 400        | API test            |
| Existing settings (without area in payload) still work | Backward compatible |

**Effort:** 15 minutes

---

### T5: Replace hardcoded `AREA = "FI"` in API routes

Replace all 9 hardcoded `"FI"` references in `app.ts` with the user's area from settings:

1. **Session-protected routes** (`/api/v1/me/chart`): use `settings.area`
2. **API key routes** (`/api/v1/price/*`): use `settings.area`
3. **Remove `const AREA = "FI"`** line

Each route that currently does `getPricesForDate(db, date, AREA)` becomes `getPricesForDate(db, date, settings.area)`.

| Detail                                                 | Acceptance                                          |
| ------------------------------------------------------ | --------------------------------------------------- |
| `const AREA = "FI"` removed from `app.ts`              | Code review                                         |
| `/api/v1/price/now` uses user's configured area        | Manual test                                         |
| `/api/v1/me/chart` uses user's configured area         | Manual test                                         |
| All routes pass `settings.area` not a hardcoded string | Grep confirms no hardcoded `"FI"` in route handlers |

**Effort:** 20 minutes

---

### T6: Public endpoint — add `?area=` query param

Update `GET /api/public/spot` in `app.ts`:

```typescript
const area = c.req.query("area")?.toUpperCase() ?? "FI";
if (!isValidAreaCode(area)) {
  return c.json({ error: "Invalid area code" }, 400);
}
```

| Detail                                         | Acceptance          |
| ---------------------------------------------- | ------------------- |
| `/api/public/spot` still returns FI by default | Backward compatible |
| `/api/public/spot?area=SE3` returns SE3 prices | API test            |
| `/api/public/spot?area=INVALID` returns 400    | API test            |

**Effort:** 10 minutes

---

### T7: Multi-area ingestion

Update `src/fetch-job.ts`:

1. Replace `DEFAULT_AREA = "FI"` with all 21 areas
2. Modify `fetchForDate` to fetch all areas in a single API call
3. Update `src/nordpool.ts` `buildUrl` to accept multiple areas (comma-separated `deliveryArea`)
4. Update `parseResponse` to return prices for all areas in the response

**Key change in `nordpool.ts`:**

```typescript
// Accept single area or array of areas
interface FetchPricesParams {
  readonly date: string;
  readonly areas: readonly string[]; // Changed from single area
}
```

The API call `deliveryArea=FI,SE1,SE2,...` returns `entryPerArea` with all areas in one response. Parse all of them.

| Detail                                              | Acceptance                                         |
| --------------------------------------------------- | -------------------------------------------------- |
| Single API call fetches all 21 areas                | Network log shows 1 HTTP request                   |
| All areas stored in `prices` table                  | `SELECT DISTINCT area FROM prices` returns 21 rows |
| Existing `fetchDayAheadPrices` signature updated    | TypeScript compiles                                |
| Backward compatible: if called with one area, works | Unit test                                          |

**Effort:** 30 minutes

---

### T8: Settings UI — area and timezone dropdowns

Update `src/ui.ts`:

1. Add area `<select>` dropdown to settings panel (between existing fields and Save button)
2. Add timezone `<select>` dropdown below area dropdown
3. Both dropdowns populated with hardcoded options from the 21 areas / 13 timezones
4. JavaScript: when area dropdown changes, auto-set timezone to area's default
5. `loadSettings` populates both dropdowns from API response
6. `saveBtn` includes `area` and `timezone` in PUT payload

**Area dropdown options:**

```
Finland (FI)
Sweden — Luleå (SE1)
Sweden — Sundsvall (SE2)
Sweden — Stockholm (SE3)
Sweden — Malmö (SE4)
Norway — Oslo (NO1)
...etc
```

**Timezone dropdown options:** 13 timezones, each with area codes that use it.

| Detail                                               | Acceptance  |
| ---------------------------------------------------- | ----------- |
| Area dropdown shows all 21 areas with readable names | E2E test    |
| Timezone dropdown shows 13 timezones                 | E2E test    |
| Changing area auto-sets timezone                     | Manual test |
| Save persists area + timezone                        | E2E test    |
| Defaults: FI selected, Europe/Helsinki selected      | Verified    |

**Effort:** 30 minutes

---

### T9: Update existing tests

- Update unit tests that reference hardcoded `"FI"` or mock `UserSettings` without `area`
- Update E2E tests if settings form structure changed
- Add new unit tests for `areas.ts` validation functions

| Detail                                 | Acceptance               |
| -------------------------------------- | ------------------------ |
| All existing unit tests pass           | `npm test` green         |
| All existing E2E tests pass            | `npm run test:e2e` green |
| New tests for area/timezone validation | Tests exist and pass     |

**Effort:** 20 minutes

---

### T10: Verification pass

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
T1 (constants) → T2 (migration) → T3 (types/settings)
                                 → T4 (settings API validation)
                                 → T5 (replace hardcoded FI)
                                 → T6 (public endpoint)
                                 → T7 (multi-area ingestion)
                                 → T8 (UI dropdowns)
                                 → T9 (tests)
                                 → T10 (verification)
```

T1–T3 are sequential foundations. T4–T8 can proceed in order after T3. T9–T10 finalize.

**Total estimated effort: ~3 hours**

---

## Risks and Mitigations

| #   | Risk                                                              | Likelihood | Impact | Mitigation                                                               |
| --- | ----------------------------------------------------------------- | :--------: | :----: | ------------------------------------------------------------------------ |
| R1  | Multi-area API response is too large or slow                      |    Low     |  Low   | Test response time; API returned ~200 KB in research — acceptable        |
| R2  | `ALTER TABLE ADD COLUMN` fails on existing production DB          |  Very Low  | Medium | SQLite supports `ALTER TABLE ADD COLUMN` natively; test with prod backup |
| R3  | Existing tests break due to missing `area` field                  |   Medium   |  Low   | Add `area: "FI"` to all test fixtures in T9                              |
| R4  | UI dropdown is confusing for users who don't know their area code |    Low     |  Low   | Display format: "Country — Region (CODE)"                                |
| R5  | `nordpool.ts` signature change breaks callers                     |   Medium   |  Low   | Update all callers in T7; TypeScript catches missing changes             |

---

## Acceptance Criteria

| #    | Criterion                                                               | Verification                          |
| ---- | ----------------------------------------------------------------------- | ------------------------------------- |
| AC1  | New user gets `area: "FI"` and `timezone: "Europe/Helsinki"` by default | Unit test                             |
| AC2  | User can change area to SE3 and see SE3 prices in chart and API         | Manual test                           |
| AC3  | Changing area in UI auto-sets timezone                                  | E2E test                              |
| AC4  | `/api/public/spot` returns FI by default, SE3 with `?area=SE3`          | API test                              |
| AC5  | `/api/v1/price/now` returns prices for user's configured area           | API test                              |
| AC6  | Ingestion fetches all 21 areas in one API call                          | Log output verification               |
| AC7  | Invalid area code in settings API returns 400                           | Unit test                             |
| AC8  | Invalid timezone in settings API returns 400                            | Unit test                             |
| AC9  | Timezone dropdown only shows 13 Nord Pool-relevant timezones            | UI inspection                         |
| AC10 | All existing tests pass without regression                              | `npm test` + `npm run test:e2e` green |

---

## Quality Gate Checklist (Pre-Merge)

- [ ] All acceptance criteria (AC1–AC10) verified
- [ ] `npm run typecheck` passes
- [ ] `npm run lint` passes
- [ ] `npm run format:check` passes
- [ ] `npm test` passes (all tests)
- [ ] `npm run test:e2e` passes (all tests)
- [ ] `npm run build` passes
- [ ] No `any` types introduced
- [ ] No code duplication
- [ ] No hardcoded `"FI"` in route handlers (only in defaults/constants)
