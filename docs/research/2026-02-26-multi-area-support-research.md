# Research Dossier: Multi-Area Nord Pool Spot Price Support

Date: 2026-02-26
Author: Research Agent
Status: **Draft** — awaiting owner review

---

## Problem Statement

The Spot-Price service currently hardcodes Finland (`FI`) as the only delivery area. Users in other Nord Pool countries (Sweden, Norway, Denmark, Baltics, Central Europe) cannot use the service. Additionally, the timezone setting exists in user settings but is not constrained to relevant timezones — offering all world timezones would be confusing and incorrect.

### User Need

> "As a user in Sweden (SE3), I want to see spot prices for my bidding area and have timestamps in my local timezone, so I can schedule flexible loads correctly."

## Target Users

| User                                 | Job-to-be-done                                                                      |
| ------------------------------------ | ----------------------------------------------------------------------------------- |
| Existing FI user                     | No change — FI remains default                                                      |
| Nordic/Baltic spot-price customer    | Select their delivery area (SE1-4, NO1-5, DK1-2, EE, LT, LV) and see correct prices |
| Central European spot-price customer | Select their area (AT, BE, FR, GER, NL, PL) and see correct prices                  |
| Home Assistant integrator            | API returns prices for their configured area with correct local timestamps          |

---

## Topic 1: Nord Pool Data Portal API — Available Delivery Areas

### Verified Live (2026-02-26)

Tested `GET https://dataportal-api.nordpoolgroup.com/api/DayAheadPrices?date=2026-02-26&market=DayAhead&deliveryArea=FI,SE1,SE2,SE3,SE4,NO1,NO2,NO3,NO4,NO5,DK1,DK2,EE,LT,LV,AT,BE,FR,GER,NL,PL&currency=EUR`

The API returned 96 quarter-hourly entries per area for **all 21 areas** in a single request:

| Area Code | Country / Region              | Confirmed |
| --------- | ----------------------------- | --------- |
| **FI**    | Finland                       | ✅        |
| **SE1**   | Sweden — Luleå (north)        | ✅        |
| **SE2**   | Sweden — Sundsvall            | ✅        |
| **SE3**   | Sweden — Stockholm            | ✅        |
| **SE4**   | Sweden — Malmö (south)        | ✅        |
| **NO1**   | Norway — Oslo (east)          | ✅        |
| **NO2**   | Norway — Kristiansand (south) | ✅        |
| **NO3**   | Norway — Trondheim (central)  | ✅        |
| **NO4**   | Norway — Tromsø (north)       | ✅        |
| **NO5**   | Norway — Bergen (west)        | ✅        |
| **DK1**   | Denmark — West (Jutland)      | ✅        |
| **DK2**   | Denmark — East (Zealand)      | ✅        |
| **EE**    | Estonia                       | ✅        |
| **LT**    | Lithuania                     | ✅        |
| **LV**    | Latvia                        | ✅        |
| **AT**    | Austria                       | ✅        |
| **BE**    | Belgium                       | ✅        |
| **FR**    | France                        | ✅        |
| **GER**   | Germany                       | ✅        |
| **NL**    | Netherlands                   | ✅        |
| **PL**    | Poland                        | ✅        |

**Note**: The API also supports `SYS` (system price) and `TEL` area codes, but these are not real delivery areas — they're system-level aggregates. We exclude them.

**Evidence**: Live API response on 2026-02-26 returned `"deliveryAreas":["FI","SE1","SE2","SE3","SE4","NO1","NO2","NO3","NO4","NO5","DK1","DK2","EE","LT","LV","AT","BE","FR","GER","NL","PL"]` and `"areaStates":[{"state":"Final","areas":[...all 21...]}]`.

### Multi-Area Single Request

The API accepts **comma-separated delivery areas** in a single request. This means we can fetch all areas in one API call instead of 21 separate calls. This is critical for efficient ingestion.

```
GET /api/DayAheadPrices?date=2026-02-26&market=DayAhead&deliveryArea=FI,SE1,SE2,...&currency=EUR
```

Response contains `multiAreaEntries[].entryPerArea` with prices for all requested areas per time slot.

---

## Topic 2: Area-to-Timezone Mapping

Each delivery area maps to exactly one IANA timezone. These are the **only timezones** the application should offer — not arbitrary world timezones.

| Area Code | Country     | IANA Timezone       | UTC Offset (winter/summer) |
| --------- | ----------- | ------------------- | -------------------------- |
| FI        | Finland     | `Europe/Helsinki`   | +2 / +3                    |
| SE1       | Sweden      | `Europe/Stockholm`  | +1 / +2                    |
| SE2       | Sweden      | `Europe/Stockholm`  | +1 / +2                    |
| SE3       | Sweden      | `Europe/Stockholm`  | +1 / +2                    |
| SE4       | Sweden      | `Europe/Stockholm`  | +1 / +2                    |
| NO1       | Norway      | `Europe/Oslo`       | +1 / +2                    |
| NO2       | Norway      | `Europe/Oslo`       | +1 / +2                    |
| NO3       | Norway      | `Europe/Oslo`       | +1 / +2                    |
| NO4       | Norway      | `Europe/Oslo`       | +1 / +2                    |
| NO5       | Norway      | `Europe/Oslo`       | +1 / +2                    |
| DK1       | Denmark     | `Europe/Copenhagen` | +1 / +2                    |
| DK2       | Denmark     | `Europe/Copenhagen` | +1 / +2                    |
| EE        | Estonia     | `Europe/Tallinn`    | +2 / +3                    |
| LT        | Lithuania   | `Europe/Vilnius`    | +2 / +3                    |
| LV        | Latvia      | `Europe/Riga`       | +2 / +3                    |
| AT        | Austria     | `Europe/Vienna`     | +1 / +2                    |
| BE        | Belgium     | `Europe/Brussels`   | +1 / +2                    |
| FR        | France      | `Europe/Paris`      | +1 / +2                    |
| GER       | Germany     | `Europe/Berlin`     | +1 / +2                    |
| NL        | Netherlands | `Europe/Amsterdam`  | +1 / +2                    |
| PL        | Poland      | `Europe/Warsaw`     | +1 / +2                    |

### Unique Timezones (10 total)

Despite 21 areas, there are only **10 unique IANA timezones**:

1. `Europe/Helsinki` (FI)
2. `Europe/Stockholm` (SE1-4)
3. `Europe/Oslo` (NO1-5)
4. `Europe/Copenhagen` (DK1-2)
5. `Europe/Tallinn` (EE)
6. `Europe/Vilnius` (LT)
7. `Europe/Riga` (LV)
8. `Europe/Vienna` (AT)
9. `Europe/Brussels` (BE)
10. `Europe/Paris` (FR)
11. `Europe/Berlin` (GER)
12. `Europe/Amsterdam` (NL)
13. `Europe/Warsaw` (PL)

Correction: **13 unique timezones**. Although many share UTC+1/+2, they are distinct IANA zones (e.g., `Europe/Berlin` ≠ `Europe/Paris` historically, and DST rules could theoretically diverge in the future).

### Design Decision: Auto-set timezone from area, allow override

When a user selects an area, the timezone should **auto-populate** to the area's default timezone. The user can then override it if they live in a different timezone than their electricity provider's area (e.g., a Finn with a Swedish electricity contract).

---

## Topic 3: Current Codebase Impact Analysis

### What's Already Multi-Area Ready

| Component             | Status     | Detail                                                                                         |
| --------------------- | ---------- | ---------------------------------------------------------------------------------------------- |
| `prices` table        | ✅ Ready   | Has `area` column, `UNIQUE(delivery_start, area)` constraint, `idx_prices_area_delivery` index |
| `user_settings` table | ⚠️ Partial | Has `timezone` column but **no `area` column**                                                 |
| `price-store.ts`      | ✅ Ready   | All queries accept `area` parameter                                                            |
| `nordpool.ts`         | ✅ Ready   | `fetchDayAheadPrices({date, area})` accepts any area code                                      |
| `calculator.ts`       | ✅ Ready   | Pure functions, no hardcoded area                                                              |
| `types.ts`            | ✅ Ready   | `HourlyPrice.area` field exists                                                                |

### What Needs to Change

| Component                     | Change Required                                                      |
| ----------------------------- | -------------------------------------------------------------------- |
| `user_settings` table         | Add `area TEXT NOT NULL DEFAULT 'FI'` column                         |
| `fetch-job.ts`                | Fetch prices for **all areas that have active users**, not just `FI` |
| `app.ts`                      | Replace hardcoded `AREA = "FI"` with user's area from settings       |
| `app.ts` (`/api/public/spot`) | Accept optional `?area=XX` query param (default `FI`)                |
| `ui.ts`                       | Add area dropdown and timezone dropdown to settings panel            |
| `user-settings.ts`            | Include `area` in UserSettings type and upsert logic                 |
| `types.ts`                    | Add `area` to `UserSettings` interface                               |

### Hardcoded `"FI"` Locations (must change)

1. `src/app.ts` line 47: `const AREA = "FI"`
2. `src/app.ts` lines 227, 231: `getPricesForDate(db, date, AREA)` in public endpoint
3. `src/app.ts` lines 299, 300: `getPricesForDate(db, date, AREA)` in `/api/v1/me/chart`
4. `src/app.ts` lines 322, 323: `getPricesForDate(db, date, AREA)` in `/api/v1/price/now`
5. `src/app.ts` line 346: `getPricesForDate(db, date, AREA)` in `/api/v1/price/today`
6. `src/app.ts` line 368: `getPricesForDate(db, date, AREA)` in `/api/v1/price/tomorrow`
7. `src/app.ts` lines 420, 421: `getPricesForDate(db, date, AREA)` in `/api/v1/price/cheapest`
8. `src/fetch-job.ts` line 5: `const DEFAULT_AREA = "FI"`
9. `src/fetch-job.ts` lines 73, 79: `fetchForDate(db, date, DEFAULT_AREA)`

---

## Topic 4: Ingestion Strategy for Multiple Areas

### Option A: Fetch All 21 Areas Always

**Single API call** fetches all areas. Store everything.

| Pro                                                    | Con                                            |
| ------------------------------------------------------ | ---------------------------------------------- |
| Simple — one cron job, one API call                    | Stores ~96 × 21 = 2,016 rows/day instead of 96 |
| Users can switch areas instantly (data already exists) | Slightly more storage (~500 KB/day)            |
| No need to track "active areas"                        |                                                |

### Option B: Fetch Only Active User Areas

Query `user_settings` for distinct areas, fetch only those.

| Pro                | Con                                             |
| ------------------ | ----------------------------------------------- |
| Minimal storage    | More complex ingestion logic                    |
| Respects API usage | New user with new area must wait for next fetch |
|                    | Must re-fetch if user changes area              |

### Option C: Fetch All Areas in Single Request, Store All

Same as Option A but explicitly noting that the Nord Pool API returns all areas in one response when you request them all. There's **no additional API cost** — it's one HTTP request either way.

### Recommendation: Option A (Fetch All 21 Areas Always)

Rationale:

- The API returns all areas in a **single HTTP request** — no extra API calls needed
- Storage is trivial: 2,016 rows × 365 days = ~735K rows/year, ~50 MB in SQLite — well within the 1 GB volume
- Users can switch areas instantly without waiting for the next fetch
- No need to track "which areas are active" — eliminates complexity
- If the request with all 21 areas becomes too large, we can split into 2-3 regional requests

---

## Topic 5: Timezone Selection UX

### Approach: Constrained Dropdown, Auto-Linked to Area

When user selects area → timezone auto-sets to area's default:

- User picks `SE3` → timezone auto-sets to `Europe/Stockholm`
- User can manually override to any of the 13 supported timezones

The timezone dropdown only offers the 13 Nord Pool-relevant timezones, not all ~400 IANA zones.

### Timezone Affects

1. **`localStart` / `localEnd`** in API responses — formatted in user's timezone
2. **Day/night transfer rate** — hour is calculated in user's timezone
3. **"Today" / "Tomorrow"** boundary — which calendar day is "today" depends on timezone
4. **Chart X-axis labels** — hours shown in user's local time

---

## Topic 6: Public Endpoint Behavior

Current `/api/public/spot` returns FI prices only. Options:

| Option                                 | Detail                                               |
| -------------------------------------- | ---------------------------------------------------- |
| A: Keep FI default, add `?area=` param | Backward compatible. `?area=SE3` returns SE3 prices. |
| B: Always FI, no param                 | Simplest but limits public endpoint usefulness       |
| C: Remove public endpoint              | Breaking change, not recommended                     |

**Recommendation: Option A** — add optional `?area=` query parameter to `/api/public/spot` with `FI` as default. This is backward compatible and useful for non-authenticated users who want to check prices for different areas.

---

## Assumptions

| #   | Assumption                                                          | Risk if Wrong                                          |
| --- | ------------------------------------------------------------------- | ------------------------------------------------------ |
| A1  | Nord Pool API continues to support all 21 areas in a single request | May need to split into multiple requests               |
| A2  | 2,016 rows/day is acceptable storage growth                         | ~50 MB/year — negligible for SQLite on 1 GB volume     |
| A3  | Users understand their electricity delivery area code               | May need country → area mapping help text in UI        |
| A4  | Area-to-timezone mapping is static                                  | If a country changes timezone policy, need code update |
| A5  | All 21 areas have consistent 15-minute resolution data              | API response confirmed this for 2026-02-26             |

## Unknowns

| #   | Unknown                                                          | Impact                                                | Mitigation                                                  |
| --- | ---------------------------------------------------------------- | ----------------------------------------------------- | ----------------------------------------------------------- |
| U1  | Whether fetching all 21 areas slows down the API response        | Low — single HTTP request, just larger JSON (~200 KB) | Test response time; split into regional requests if >5s     |
| U2  | Whether some areas have different electricity tax/VAT structures | Settings already support custom tax/VAT per user      | Document that users must configure their own tax/VAT values |
| U3  | Whether SYS and TEL area codes are useful to any user            | Excluded for now                                      | Can add later if requested                                  |

---

## Evidence Sufficiency Assessment

**Confidence: HIGH for proceeding to planning.**

- All 21 delivery areas **verified live** with real API data (2026-02-26)
- Multi-area single-request capability **confirmed** — no extra API calls needed
- Current database schema **already supports** multi-area storage
- Area-to-timezone mapping derived from standard IANA timezone database
- Codebase impact analysis identifies all 9 hardcoded `"FI"` locations
- Storage impact is trivial (~50 MB/year for all areas)

---

## Summary of Recommendations

| Decision                     | Choice                                                          | Confidence |
| ---------------------------- | --------------------------------------------------------------- | ---------- |
| Supported areas              | All 21 Nord Pool delivery areas                                 | High       |
| Ingestion strategy           | Fetch all 21 areas in single API call, store all                | High       |
| Area selection               | Dropdown in user settings, default FI                           | High       |
| Timezone selection           | Auto-set from area, allow override, 13 supported timezones only | High       |
| Public endpoint              | Add optional `?area=` param, default FI                         | High       |
| New migration                | Add `area` column to `user_settings`                            | High       |
| Timezone dropdown constraint | Only 13 Nord Pool-relevant IANA timezones                       | High       |
