# Delivery Roadmap

Date: 2026-02-24
Owner: Repository Owner + Agent
Status: Active

## Purpose

- Provide a high-level milestone view so any new agent can see sequence, dependencies, and current checkpoint quickly.

## Milestones

| #   | Milestone                           |   State    | Est. hours | Dependencies                  |
| --- | ----------------------------------- | :--------: | :--------: | :---------------------------- |
| M0  | Project Scaffolding                 | 🔲 Pending |    2–3     | —                             |
| M1  | Database Schema & Migrations        | 🔲 Pending |     2      | M0                            |
| M2  | Price Ingestion (Nord Pool)         | 🔲 Pending |     3      | M1                            |
| M3  | Total Price Calculation Engine      | 🔲 Pending |     3      | M1 (can parallel M2, M4)      |
| M4  | Auth & API Key System (Better Auth) | 🔲 Pending |     3      | M1 (can parallel M2, M3)      |
| M5  | REST API Endpoints                  | 🔲 Pending |    2–3     | M3, M4                        |
| M6  | Web UI (Minimal)                    | 🔲 Pending |    4–5     | M4, M5                        |
| M7  | Deployment & Operations (Railway)   | 🔲 Pending |     2      | M6 (full), M0 (partial setup) |

## Critical path

```
M0 → M1 → M2 → M3 → M5 → M7
              ↘ M4 → M5 ↗
                  ↘ M6 ↗
```

Critical sequence: **M0 → M1 → M3 → M5 → M7** (~12–14 hours)
With parallelism: **~6–7 sessions** total

## Dependencies

| Milestone | Depends on | Can parallel with |
| --------- | ---------- | ----------------- |
| M0        | —          | —                 |
| M1        | M0         | —                 |
| M2        | M1         | M4                |
| M3        | M1         | M2, M4            |
| M4        | M1         | M2, M3            |
| M5        | M3, M4     | —                 |
| M6        | M4, M5     | M7 setup          |
| M7        | M6 (full)  | M6                |

## How to use

- Update milestone states when project phase advances.
- Keep tactical work out of this file; place it in `docs/plans/NEXT-ACTIONS.md`.
