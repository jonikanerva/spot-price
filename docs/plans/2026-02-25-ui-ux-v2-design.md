# Design: UI/UX v2 — Developer Dark Mode

Date: 2026-02-25
Author: RPI Orchestrator (brainstorming skill)
Status: **Approved** by owner

---

## Goal

Redesign the Spot Price web UI into a polished, developer-friendly dark-mode interface with hover-interactive charts, streamlined single-API-key management, and E2E smoke tests to prevent UI regressions.

## Decisions Made

| Decision                 | Choice                                    | Rationale                                          |
| ------------------------ | ----------------------------------------- | -------------------------------------------------- |
| Chart library            | Vanilla SVG (extend current)              | Zero dependencies, full control, Railway aesthetic |
| Frontend architecture    | Single HTML template (`renderHomePage()`) | Simplest, no build step for frontend               |
| Chart interactivity      | Hover tooltip (time + price)              | Better UX for price inspection                     |
| Chart axes               | X (hours) + Y (c/kWh) labels              | Professional appearance                            |
| Settings UX              | Always-editable input fields              | Simplest, no view/edit mode toggle                 |
| API key model            | Single key per user, always visible       | Eliminates name, list, hide/reveal complexity      |
| Visual style             | Railway.com-inspired dark mode only       | Developer-friendly aesthetic                       |
| UI regression prevention | Playwright E2E smoke tests                | Catches broken buttons, missing charts             |

---

## 1. Landing Page (unauthenticated)

### Layout

```
┌─────────────────────────────────────────────────┐
│  Spot Price              [developer tagline]     │
├─────────────────────────────────────────────────┤
│  ┌───────────┐ ┌───────────┐ ┌────────────────┐│
│  │ username  │ │ password  │ │Login or Signup ││
│  └───────────┘ └───────────┘ └────────────────┘│
│  (status message)                               │
├─────────────────────────────────────────────────┤
│  Spot price — today + tomorrow                  │
│  ┌─────────────────────────────────────────────┐│
│  │ c/kWh │                                     ││
│  │  15   │    ~~~line chart~~~                  ││
│  │  10   │                                     ││
│  │   5   │                                     ││
│  │───────┼─────────────────────────────────────││
│  │       00   06   12   18   00   06   12      ││
│  │  ● Today (cyan)   ● Tomorrow (violet)       ││
│  └─────────────────────────────────────────────┘│
│  [hover: "14:15 — 8.42 c/kWh"]                 │
└─────────────────────────────────────────────────┘
```

### Behavior

- Login fields: `username` (a-z0-9\_-) + `password` + one button "Login or Signup"
- If username exists: sign in. If not: create account, then sign in.
- No email collected — username maps to internal `username@spot.internal` for Better Auth.
- Public chart: 15-min spot prices from `/api/public/spot`, c/kWh.
- Today = cyan line, tomorrow = violet line (when available after ~14:00 EET).
- Hover tooltip shows exact time + price.
- Mobile: fields stack vertically.

---

## 2. Dashboard (authenticated)

### Layout

```
┌──────────────────────────────────────────────────────┐
│  Spot Price     [Dashboard] [API] [Logout]    user123│
├──────────────────────────────────────────────────────┤
│  ┌─────────────────────────────┐ ┌─────────────────┐ │
│  │ Total price chart           │ │ Settings        │ │
│  │ (your contract settings)    │ │                 │ │
│  │ c/kWh │                     │ │ Margin    [0.5] │ │
│  │  20   │  ~~~chart~~~        │ │ Day xfer  [2.5] │ │
│  │  15   │                     │ │ Night xfer[1.2] │ │
│  │  10   │                     │ │ Tax       [2.79]│ │
│  │───────┼─────────────────    │ │ VAT %     [25.5]│ │
│  │       00  06  12  18  00    │ │ Night hrs [22-7]│ │
│  │ ● Today  ● Tomorrow        │ │ [Save settings] │ │
│  └─────────────────────────────┘ └─────────────────┘ │
└──────────────────────────────────────────────────────┘
```

### Behavior

- Left panel: total price chart (spot + margin + transfer + tax + VAT per user settings).
- Right panel: always-editable settings fields. Save button triggers recalculation + chart refresh.
- Top bar: logo, nav buttons (Dashboard active, API, Logout), username display.
- Chart: same vanilla SVG with axes + hover tooltip, but showing total price.
- Mobile: chart stacks above settings.

---

## 3. API Panel (authenticated, replaces chart area)

### Layout

```
┌──────────────────────────────────────────────────────┐
│  Spot Price     [Dashboard] [API*] [Logout]   user123│
├──────────────────────────────────────────────────────┤
│  ┌────────────────────────────────────────────────┐  │
│  │ API                                            │  │
│  │                                                │  │
│  │ Your API key:                                  │  │
│  │ ┌────────────────────────────────────────────┐ │  │
│  │ │ sp_abc123def456ghi789jkl012mno345          │ │  │
│  │ └────────────────────────────────────────────┘ │  │
│  │ [Regenerate API key]                           │  │
│  │                                                │  │
│  │ Usage examples:                                │  │
│  │ ┌────────────────────────────────────────────┐ │  │
│  │ │ # Current price                            │ │  │
│  │ │ curl -sS \                                  │ │  │
│  │ │   -H "Authorization: Bearer sp_abc..." \    │ │  │
│  │ │   https://spot.calmdonut.com/api/v1/...    │ │  │
│  │ │                                            │ │  │
│  │ │ # Cheapest 3-hour window                   │ │  │
│  │ │ curl -sS \                                  │ │  │
│  │ │   -H "Authorization: Bearer sp_abc..." \    │ │  │
│  │ │   .../api/v1/price/cheapest?duration=180   │ │  │
│  │ │                                            │ │  │
│  │ │ # Today's prices                           │ │  │
│  │ │ curl -sS \                                  │ │  │
│  │ │   -H "Authorization: Bearer sp_abc..." \    │ │  │
│  │ │   .../api/v1/price/today                   │ │  │
│  │ └────────────────────────────────────────────┘ │  │
│  └────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────┘
```

### API Key Model (simplified)

| Rule           | Detail                                                    |
| -------------- | --------------------------------------------------------- |
| Keys per user  | Exactly 1 at a time                                       |
| Visibility     | Always shown in API panel (stored retrievably in DB)      |
| Creation       | Auto-created on first visit to API panel (if none exists) |
| Regeneration   | "Regenerate API key" button: deletes old, creates new     |
| Name field     | None — removed entirely                                   |
| List/delete UI | None — just the one key + regenerate                      |

### Backend Changes Required

Current implementation hashes API keys (one-way). New model needs:

- Store key **encrypted** (AES-256-GCM with server secret) instead of hashed, so it can be decrypted and shown.
- OR store key **plaintext** in DB (simpler; acceptable for hobby project where DB is server-local SQLite).
- Single-key constraint: `api_keys` table gets `UNIQUE(user_id)` or we delete old before inserting new.

**Recommendation:** Store plaintext in DB. This is a hobby project with server-local SQLite. The key itself is a random bearer token, not a password. Encrypting adds complexity without meaningful security gain when the DB is on the same volume as the app.

### Usage Examples Content

The curl examples use the user's **actual API key** (not a placeholder), so they're copy-pasteable.

---

## 4. Visual Style (Railway-inspired dark mode)

### Color System

```css
--bg: #0b0f1a /* page background */ --bg-soft: #111729 /* elevated surfaces */
  --panel: #141b2f /* card backgrounds */ --panel-2: #1a223a
  /* card gradient end */ --text: #e6edf7 /* primary text */ --muted: #96a4c2
  /* secondary text */ --border: #2b3655 /* borders */ --accent: #5dd5ff
  /* primary accent (cyan) */ --accent-2: #7b6bff
  /* secondary accent (violet) */ --ok: #41d39d /* success */ --err: #ff6b8a
  /* error */;
```

### Typography

- Primary: `"JetBrains Mono", "IBM Plex Mono", monospace`
- Code blocks: same font, darker background (`#0d1323`)
- Sizes: h1 1.65rem, h2 1rem, body 0.9rem, small/muted 12px

### Components

- Cards: gradient background, 1px border, 14px radius, drop shadow
- Buttons (primary): gradient `accent → accent-2`, bold text, 10px radius
- Buttons (secondary): dark bg, border, light text
- Inputs: dark bg (`#0f1528`), border, cyan focus ring
- Code blocks (`<pre>`): dark bg, border, monospace, pre-wrap

### Responsive Breakpoints

- Desktop (>980px): side-by-side layout (chart + settings)
- Mobile (<=980px): stacked layout (chart above settings, login fields stacked)

---

## 5. Chart Specification (Vanilla SVG)

### Features

| Feature       | Detail                                                                 |
| ------------- | ---------------------------------------------------------------------- |
| Type          | Line chart (polyline path)                                             |
| Resolution    | 15-minute intervals                                                    |
| X-axis        | Hour labels: 00, 06, 12, 18, 00 (tomorrow boundary marked)             |
| Y-axis        | Price labels in c/kWh (auto-scaled, 3-5 ticks)                         |
| Today line    | Cyan (`#5dd5ff`), 2px stroke                                           |
| Tomorrow line | Violet (`#7b6bff`), 2px stroke (when available)                        |
| Hover tooltip | Vertical crosshair line + floating label: "14:15 — 8.42 c/kWh"         |
| Padding       | 40px left (Y-axis labels), 20px bottom (X-axis labels), 10px top/right |
| Background    | `#0e1426` with grid lines at Y-tick positions                          |
| Legend        | Below chart: colored dots + "Today" / "Tomorrow"                       |

### Tooltip Behavior

- On `mousemove` over SVG: find nearest data point by X position
- Show vertical line at that X position
- Floating label near cursor: `"HH:MM — X.XX c/kWh"` (or total for dashboard chart)
- On `mouseleave`: hide tooltip
- Touch: `touchmove` maps to same behavior

---

## 6. E2E Smoke Tests (Playwright)

### Purpose

Prevent UI regressions where interactive elements silently break (like buttons not working).

### Test Scenarios

| #   | Test                  | Assertion                                                    |
| --- | --------------------- | ------------------------------------------------------------ |
| 1   | Landing loads         | Page title contains "Spot Price"                             |
| 2   | Public chart renders  | SVG contains at least one `<path>` element                   |
| 3   | Login flow            | Enter username + password → click button → dashboard appears |
| 4   | Dashboard chart loads | Total chart SVG contains `<path>` elements                   |
| 5   | Settings save         | Change a value → Save → status shows "saved"                 |
| 6   | API panel navigation  | Click API → API key is visible                               |
| 7   | Regenerate key        | Click Regenerate → key value changes                         |
| 8   | Logout                | Click Logout → landing page reappears                        |

### Implementation

- Playwright with `@playwright/test`
- Runs against local dev server (`localhost:3000`)
- Added to CI as `npm run test:e2e`
- Separate from unit tests (`npm test` remains fast)

---

## Scope Boundaries

### In scope

- Redesign `src/ui.ts` template with above layout and style
- Add SVG chart axes and hover tooltip
- Simplify API key model (single key, always visible, regenerate)
- Backend: store API key retrievably, enforce single-key-per-user
- Playwright E2E smoke tests
- Responsive mobile layout

### Out of scope

- Light mode / theme switcher
- Account recovery / password reset
- Multi-language support
- Chart animations
- Separate frontend build system
