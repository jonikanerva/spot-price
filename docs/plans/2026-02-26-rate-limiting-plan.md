# Implementation Plan: Rate Limiting & Abuse Protection

Date: 2026-02-26
Author: RPI Orchestrator
Status: Draft — awaiting owner review
Research basis: `docs/research/2026-02-26-rate-limiting-research.md` (Approved 2026-02-26)

---

## Goal

Add rate limiting, signup abuse protection, and Railway cost controls to prevent service abuse and hosting cost spikes. Keep it minimal — one new dependency, ~35 lines of config, net code reduction by replacing custom rate limiter.

## Scope Boundaries

**In scope:**

- Install `hono-rate-limiter` (0 transitive deps)
- Global IP rate limit (120 req/min, all routes except `/health`)
- Login/signup rate limit (10 req/15min per IP)
- API key rate limit (60 req/min per key, replace custom code)
- User cap (100 max users, checked at signup)
- IP extraction utility (`X-Real-IP` → `X-Forwarded-For` fallback)
- Remove existing custom rate limiter from `src/middleware.ts`
- Unit tests for user cap logic
- Document Railway cost control settings (hard limit $10, alert $7)

**Out of scope:**

- Cloudflare integration
- SQLite-backed rate limiting
- Per-route rate limits for public/session endpoints (covered by global IP limit)
- CAPTCHA or proof-of-work
- Rate limit dashboard/monitoring UI

---

## Tasks

### T1: Install dependency

| Detail                          | Acceptance                             |
| ------------------------------- | -------------------------------------- |
| `npm install hono-rate-limiter` | Package in `package.json` dependencies |

**Effort:** 1 minute

---

### T2: IP extraction utility

Create a pure function for extracting client IP from Hono context.

```typescript
// src/middleware.ts
const getClientIp = (c: Context): string =>
  c.req.header("x-real-ip") ??
  c.req.header("x-forwarded-for")?.split(",")[0]?.trim() ??
  "unknown";
```

| Detail                                                              | Acceptance        |
| ------------------------------------------------------------------- | ----------------- |
| Prefers `X-Real-IP` (Railway edge), falls back to `X-Forwarded-For` | Returns string IP |
| Pure function, no side effects                                      | Testable          |

**Effort:** 5 minutes

---

### T3: Global IP rate limiter

Apply `hono-rate-limiter` as global middleware, before all routes.

```typescript
import { rateLimiter } from "hono-rate-limiter";

// Global: 120 req/min per IP
app.use(
  rateLimiter({
    windowMs: 60_000,
    limit: 120,
    keyGenerator: (c) => getClientIp(c),
    skip: (c) => c.req.path === "/health",
    standardHeaders: "draft-6",
    message: { error: "Too many requests. Try again later." },
  }),
);
```

| Detail                                    | Acceptance                                 |
| ----------------------------------------- | ------------------------------------------ |
| Applied before all route handlers         | `RateLimit-*` headers present in responses |
| Skips `/health`                           | Health check always returns 200            |
| Returns 429 with JSON error when exceeded | Verified with test                         |

**Effort:** 10 minutes

---

### T4: Login/signup rate limiter

Apply stricter rate limit specifically on the login/signup endpoint.

```typescript
// Login: 10 req/15min per IP
app.use(
  "/api/session/login-or-signup",
  rateLimiter({
    windowMs: 15 * 60_000,
    limit: 10,
    keyGenerator: (c) => `login:${getClientIp(c)}`,
    standardHeaders: "draft-6",
    message: { error: "Too many login attempts. Try again in 15 minutes." },
  }),
);
```

| Detail                                                            | Acceptance                         |
| ----------------------------------------------------------------- | ---------------------------------- |
| Applied before the login handler                                  | 11th attempt in 15 min returns 429 |
| Uses IP as key (not username — attacker might try many usernames) | Correct key generation             |
| Different key prefix from global limiter                          | No collision with global limiter   |

**Effort:** 10 minutes

---

### T5: API key rate limiter (replace custom)

Replace the existing custom `rateLimit` middleware with a `hono-rate-limiter` instance.

```typescript
// API key: 60 req/min per key
const apiKeyRateLimit = rateLimiter({
  windowMs: 60_000,
  limit: 60,
  keyGenerator: (c) => `apikey:${c.get("userId")}`,
  standardHeaders: "draft-6",
  message: { error: "Rate limit exceeded. Try again later." },
});
```

| Detail                                                                              | Acceptance                       |
| ----------------------------------------------------------------------------------- | -------------------------------- |
| Replaces `rateLimit` export in `src/middleware.ts`                                  | Custom rate limiter code removed |
| Same behavior: 60 req/min per API key                                               | Existing tests still pass        |
| Adds standard `RateLimit-*` headers                                                 | Headers present in response      |
| Remove `rateLimitStore`, `WINDOW_MS`, `MAX_REQUESTS`, `cleanExpired`, `setInterval` | Net code reduction               |

**Effort:** 15 minutes

---

### T6: User cap (100 max)

Add a user count check in the signup path of `POST /api/session/login-or-signup`.

```typescript
// In signup path (when existingUserId is null):
const MAX_USERS = 100;
const userCount = (
  db.prepare('SELECT COUNT(*) as count FROM "user"').get() as { count: number }
).count;
if (userCount >= MAX_USERS) {
  return c.json({ error: "Registration is currently closed." }, 403);
}
```

| Detail                                         | Acceptance                                      |
| ---------------------------------------------- | ----------------------------------------------- |
| Check runs only on signup path (not login)     | Existing users can still log in                 |
| Returns 403 with clear message                 | Not 429 (it's not rate limiting, it's a policy) |
| `MAX_USERS` is a named constant                | Easy to change later                            |
| Unit test: signup fails when user count >= 100 | Test with mock DB                               |

**Effort:** 15 minutes

---

### T7: Remove custom rate limiter code

Delete from `src/middleware.ts`:

- `RateLimitEntry` interface
- `rateLimitStore` Map
- `WINDOW_MS` and `MAX_REQUESTS` constants
- `cleanExpired` function
- `setInterval(cleanExpired, ...)` side effect
- `rateLimit` middleware function

Replace with `hono-rate-limiter` export.

| Detail                                       | Acceptance         |
| -------------------------------------------- | ------------------ |
| `src/middleware.ts` is shorter               | Net code reduction |
| No `setInterval` side effect at module level | Cleaner module     |
| All existing tests pass                      | `npm test` green   |

**Effort:** 10 minutes

---

### T8: Update existing tests

Update `src/api-routes.test.ts` and `src/app.test.ts` if they reference the custom rate limiter. Ensure rate limit headers are tested.

| Detail                                                   | Acceptance             |
| -------------------------------------------------------- | ---------------------- |
| Existing rate limit test updated for new behavior        | Tests pass             |
| Add test: login rate limit returns 429 after 10 attempts | Test exists and passes |
| Add test: user cap returns 403 when DB has >= 100 users  | Test exists and passes |

**Effort:** 20 minutes

---

### T9: Railway cost controls (manual, dashboard)

Document the settings to configure in Railway dashboard:

1. **Hard usage limit**: Set to $10 in Workspace Settings → Usage
2. **Email alert**: Set to $7
3. **Service resource limits** (optional): CPU 1 vCPU, RAM 512 MB

| Detail                                            | Acceptance           |
| ------------------------------------------------- | -------------------- |
| Settings documented in this plan                  | Owner can apply them |
| Not automated — requires Railway dashboard access | Owner action item    |

**Effort:** 5 minutes (manual)

---

### T10: Verification pass

Run all checks and verify the full protection stack works.

| Check      | Command                | Expected                  |
| ---------- | ---------------------- | ------------------------- |
| TypeScript | `npm run typecheck`    | Pass                      |
| Lint       | `npm run lint`         | Pass                      |
| Format     | `npm run format:check` | Pass                      |
| Unit tests | `npm test`             | Pass (all existing + new) |
| E2E tests  | `npm run test:e2e`     | Pass (all 11)             |
| Build      | `npm run build`        | Pass                      |

**Effort:** 10 minutes

---

## Task Order

```
T1 (install) → T2 (IP util) → T3 (global limiter) → T4 (login limiter)
                                                    → T5 (API key limiter, replace custom)
                                                    → T6 (user cap)
                             → T7 (remove old code)
                             → T8 (update tests)
                             → T10 (verification)
```

All tasks are sequential in practice (single file changes overlap). Total estimated effort: **~1.5 hours**.

---

## Risks and Mitigations

| #   | Risk                                                           | Likelihood | Impact | Mitigation                                                                                    |
| --- | -------------------------------------------------------------- | :--------: | :----: | --------------------------------------------------------------------------------------------- |
| R1  | `hono-rate-limiter` API incompatible with current Hono version |    Low     | Medium | Check compatibility before install; v0.5.3 supports Hono v4                                   |
| R2  | Global rate limit blocks legitimate burst usage                |    Low     |  Low   | 120/min is generous; monitor after deploy                                                     |
| R3  | `X-Real-IP` not available in dev (localhost)                   |   Medium   |  Low   | Fallback to `X-Forwarded-For` → `"unknown"` works for dev; all localhost requests share limit |
| R4  | Rate limit state lost on redeploy                              |  Expected  |  None  | Ephemeral state is fine; brief window of no limiting is acceptable                            |

---

## Acceptance Criteria

| #   | Criterion                                                  | Verification               |
| --- | ---------------------------------------------------------- | -------------------------- |
| AC1 | Global IP limit returns 429 after 120 req/min from same IP | Manual test with curl loop |
| AC2 | Login limit returns 429 after 10 attempts in 15 min        | Unit test                  |
| AC3 | API key limit returns 429 after 60 req/min                 | Existing test updated      |
| AC4 | User cap returns 403 when 100 users exist                  | Unit test                  |
| AC5 | `/health` is never rate limited                            | Verify `skip` works        |
| AC6 | Standard `RateLimit-*` headers in responses                | Check response headers     |
| AC7 | All existing tests pass                                    | `npm test` green           |
| AC8 | Custom rate limiter code fully removed                     | Code review                |
| AC9 | Railway cost controls documented                           | Plan includes instructions |

---

## Quality Gate Checklist (Pre-Merge)

- [ ] All acceptance criteria (AC1–AC9) verified
- [ ] `npm run typecheck` passes
- [ ] `npm run lint` passes
- [ ] `npm run format:check` passes
- [ ] `npm test` passes (all tests)
- [ ] `npm run test:e2e` passes (all 11 tests)
- [ ] `npm run build` passes
- [ ] No `any` types introduced
- [ ] No code duplication
- [ ] Net code reduction confirmed (custom rate limiter removed)
