# Research Dossier: Rate Limiting & Abuse Protection

Date: 2026-02-26
Author: Research Agent
Status: **Approved** — owner GO on 2026-02-26

---

## Problem Statement

The Spot-Price service is a hobby-scale Finnish electricity price API running on Railway Hobby plan ($5-7/mo). It currently has **one narrow rate limit** (60 req/min per API key on `/api/v1/price/*` routes) but **no protection** on:

- Login/signup endpoint (can be abused to fill the user table indefinitely)
- Public endpoints (can be hammered without authentication)
- Session-protected endpoints (no per-session rate limit)
- Global level (no IP-based protection against any single source overwhelming the server)

### Threat Model

| Threat                                           | Likelihood |               Impact               | Current Protection     |
| ------------------------------------------------ | :--------: | :--------------------------------: | ---------------------- |
| Script-kiddie signup spam (fill user table)      |   Medium   | High (DB growth, auth table bloat) | **None**               |
| Rogue Home Assistant automation polling too fast |   Medium   |   Medium (CPU/egress cost spike)   | 60 req/min per API key |
| Bot hammering `/api/public/spot`                 |   Medium   |      Medium (CPU/egress cost)      | **None**               |
| Credential stuffing on login endpoint            |    Low     |  Medium (CPU from bcrypt hashing)  | **None**               |
| Volumetric DDoS (L3/L4)                          |    Low     |        High (service down)         | Railway L4 mitigation  |
| Application-layer flood (L7 DDoS)                |    Low     |  High (cost spike, service down)   | **None**               |

### Cost Impact

Railway charges per-use: CPU at $20/vCPU/month, RAM at $10/GB/month, egress at $0.05/GB. A sustained 100 req/s flood for 24 hours could push CPU usage from ~$2/mo to ~$15-20/mo and egress from ~$0.05/mo to several dollars. The Hobby plan includes $5 of usage — anything above that is billed. Railway's **hard usage limit** feature (minimum $10) can cap total spend, but the service goes offline when the limit is hit.

**Evidence**: Railway pricing docs confirm pay-per-use model with CPU at $0.000463/vCPU/minute and egress at $0.000000047683716/KB. Hard usage limits available with $10 minimum. [Source: docs.railway.com/pricing/plans, docs.railway.com/pricing/cost-control]

---

## Topic 1: Hono Rate Limiting Ecosystem

### Option A: `hono-rate-limiter` (Community Package)

**Verified** — npm package and documentation reviewed.

| Attribute              | Detail                                                                                                                           |
| ---------------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| **Package**            | `hono-rate-limiter` v0.5.3                                                                                                       |
| **npm downloads**      | 113,034 weekly                                                                                                                   |
| **GitHub stars**       | 620                                                                                                                              |
| **License**            | MIT                                                                                                                              |
| **Dependencies**       | 0 (zero dependencies)                                                                                                            |
| **Default store**      | In-memory (`MemoryStore`)                                                                                                        |
| **Key features**       | `keyGenerator`, `windowMs`, `limit`, `skip`, `skipFailedRequests`, standard rate limit headers (draft-6/draft-7), custom handler |
| **Hono compatibility** | First-class — designed specifically for Hono                                                                                     |
| **TypeScript**         | Built-in type declarations                                                                                                       |

**API surface** (from documentation):

```typescript
import { rateLimiter } from "hono-rate-limiter";

app.use(
  rateLimiter({
    windowMs: 15 * 60 * 1000,
    limit: 100,
    keyGenerator: (c) => c.req.header("x-forwarded-for") ?? "",
    skip: (c) => c.req.path === "/health",
    message: { error: "Rate limit exceeded" },
    standardHeaders: "draft-6",
  }),
);
```

**Evidence**: npm page shows v0.5.3, 113,034 weekly downloads, 0 dependencies, MIT license. Documentation at honohub.dev/docs/rate-limiter confirms all configuration options. GitHub shows 620 stars, 247 commits, 10 contributors. [Source: npmjs.com/package/hono-rate-limiter, honohub.dev/docs/rate-limiter]

### Option B: Custom In-Memory Rate Limiter (Current Approach, Extended)

The project already has a working custom rate limiter in `src/middleware.ts`. It uses a `Map<string, RateLimitEntry>` with 60-second windows and periodic cleanup. Extending for all use cases would require ~80-120 additional lines including a generic factory, IP extraction, multiple instances, and standard rate limit headers.

### Option C: Hono Built-in Rate Limiting

**Hono does not have built-in rate limiting middleware.** The official Hono middleware list includes logger, CORS, JWT, basic auth, cookie, etc. — but no rate limiter.

**Evidence**: Reviewed Hono v4.7.0 middleware documentation. No rate limiting middleware exists in the core package. [Source: hono.dev/docs/middleware/builtin]

### Comparison: Rate Limiter Options

| Criterion          | `hono-rate-limiter` | Custom (extend current)  | Hono built-in |
| ------------------ | :-----------------: | :----------------------: | :-----------: |
| Exists             |         Yes         |      Yes (partial)       |      No       |
| New dependencies   |  1 (0 transitive)   |            0             |      N/A      |
| Standard headers   |   Yes (draft-6/7)   |          Manual          |      N/A      |
| Lines to add       |   ~20-30 (config)   | ~80-120 (implementation) |      N/A      |
| Weekly downloads   |        113k         |           N/A            |      N/A      |
| Maintenance burden |   Low (community)   |      Medium (ours)       |      N/A      |

### Recommendation: `hono-rate-limiter`

Zero-dependency, 113k weekly downloads, MIT licensed, designed specifically for Hono, provides standard rate limit headers out of the box. Writing a custom implementation would duplicate its functionality with more code and no standard headers.

---

## Topic 2: IP Extraction on Railway

| Header            | Content                                    | Reliability                             |
| ----------------- | ------------------------------------------ | --------------------------------------- |
| `X-Real-IP`       | Client's remote IP address                 | **Recommended** — set by Railway's edge |
| `X-Forwarded-For` | May contain multiple IPs (client, proxies) | Less reliable — can be spoofed          |

**Current deployment** uses Railway's domain directly, so `X-Real-IP` should be the actual client IP.

**Evidence**: Railway networking docs confirm `X-Real-IP` header for client's remote IP. [Source: docs.railway.com/networking/public-networking/specs-and-limits]

---

## Topic 3: Railway Platform Protections

| Protection                        | Available                                                    |
| --------------------------------- | ------------------------------------------------------------ |
| L3/L4 DDoS mitigation             | Yes                                                          |
| L7 (application layer) protection | No — "we do not provide protection on the application layer" |
| WAF                               | No — "recommend using Cloudflare alongside Railway"          |
| Rate limiting at edge             | No                                                           |
| Max concurrent connections        | 10,000 per service                                           |
| Max HTTP RPS                      | ~11,000 per domain                                           |

### Railway Cost Control Features

| Feature                         | Detail                                              |
| ------------------------------- | --------------------------------------------------- |
| **Usage hard limit**            | Minimum $10 — shuts down all workloads when reached |
| **Email alert (soft limit)**    | Configurable — sends email when threshold reached   |
| **Resource limits per service** | Can cap max CPU and RAM per service                 |

**Evidence**: Railway cost control docs confirm hard limit feature with $10 minimum. [Source: docs.railway.com/pricing/cost-control]

---

## Topic 4: In-Memory vs SQLite-Backed Rate Limiting

**Recommendation: In-Memory.** Rate limit state is ephemeral by nature — doesn't matter if it resets on redeploy. SQLite writes for every request would add I/O load. `hono-rate-limiter`'s default `MemoryStore` handles this correctly. Single-instance means no shared state needed.

---

## Topic 5: Cloudflare as Alternative

Railway recommends Cloudflare for L7 protection. Free plan offers unlimited DDoS protection, WAF, and rate limiting. **Not needed for MVP** — adds operational complexity. Document as mitigation option for real attacks.

---

## Protection Layers Evaluated

### Layer 1: Global IP Rate Limit

120 req/min per IP, all routes except `/health`. A legitimate HA instance polls 4 req/min; a user browsing makes 20-30 in a burst. 120/min stops scripts doing 1000 req/min.

### Layer 2: Login/Signup Rate Limit

10 attempts per 15 min per IP. Legitimate user might mistype 3-5 times. Bot limited to 40/hour per IP. Combined with user cap, sufficient.

### Layer 3: API Key Rate Limit (existing, migrated)

60 req/min per API key. Replace custom code with `hono-rate-limiter` instance.

### Layer 4: Database Growth Limit (User Cap)

Hard cap of 100 users via `SELECT COUNT(*)` before signup. ~5 lines of code. Deterministic protection against unbounded DB growth.

### Layer 5: Railway Cost Controls

Hard limit $10, email alert $7. Ultimate safety net — max bill $15/month.

---

## Alternatives Comparison

| Criterion                   | A: Minimal | B: Comprehensive | C: Zero Deps |
| --------------------------- | :--------: | :--------------: | :----------: |
| New dependencies            |     1      |        1         |      0       |
| Lines of new code           |    ~35     |       ~65        |     ~125     |
| Standard rate limit headers |    Yes     |       Yes        |    Manual    |
| Covers login abuse          |    Yes     |       Yes        |     Yes      |
| User cap                    |    Yes     |       Yes        |     Yes      |
| Railway cost controls       |    Yes     |       Yes        |     Yes      |
| Maintenance burden          |    Low     |      Medium      |    Higher    |

---

## Assumptions

| #   | Assumption                                                | Risk if Wrong                                          |
| --- | --------------------------------------------------------- | ------------------------------------------------------ |
| A1  | `X-Real-IP` reflects true client IP on Railway            | IP-based rate limiting ineffective                     |
| A2  | Service remains single-instance                           | In-memory rate limiting wouldn't work across instances |
| A3  | Tens of users is expected scale                           | User cap of 100 might be hit                           |
| A4  | `hono-rate-limiter` MemoryStore handles cleanup correctly | Memory leak; mitigated by Railway redeploys            |
| A5  | Attackers are unsophisticated                             | Distributed attacks would bypass IP-based limiting     |

## Unknowns

| #   | Unknown                                                | Mitigation                                                         |
| --- | ------------------------------------------------------ | ------------------------------------------------------------------ |
| U1  | `X-Real-IP` trustworthiness with CDN in front          | Currently no CDN; switch to `CF-Connecting-IP` if Cloudflare added |
| U2  | `hono-rate-limiter` behavior under high concurrency    | Low risk — Map operations are synchronous in Node.js               |
| U3  | Railway's 11,000 RPS edge limit as implicit protection | Treat as bonus, not primary defense                                |

---

## Summary of Recommendations

| Decision                    | Choice                                    | Confidence |
| --------------------------- | ----------------------------------------- | :--------: |
| Rate limiter library        | `hono-rate-limiter` (v0.5.3, 0 deps)      |    High    |
| Rate limiter store          | In-memory (default MemoryStore)           |    High    |
| Global protection           | IP-based, 120 req/min, skip `/health`     |    High    |
| Login protection            | IP-based, 10 req/15min                    |    High    |
| API key protection          | 60 req/min per key (replace custom code)  |    High    |
| Database growth control     | User cap of 100 in signup handler         |    High    |
| Railway cost control        | Hard limit $10, email alert $7            |    High    |
| Cloudflare                  | Not needed now; document as future option |    High    |
| **Recommended alternative** | **A: Minimal** (~35 lines, 1 dependency)  |    High    |
