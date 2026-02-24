# Next Actions (Execution Queue)

Date: 2026-02-24
Owner: Repository Owner + Agent
Status: Active

## Rules

- Keep exactly 1 item `In progress`.
- Keep maximum 3 active items total (`In progress` + `Ready`).
- Every item includes owner, acceptance check, and target checkpoint.
- Move completed items to the history section with date and evidence link.

## Active queue

| #   | Action                                                                                           | Owner | Status          | Acceptance check                                                                             | Target            |
| --- | ------------------------------------------------------------------------------------------------ | ----- | --------------- | -------------------------------------------------------------------------------------------- | ----------------- |
| 1   | M6 closure: capture dashboard screenshots + verify production has latest UI and quarter-hour fix | Agent | **In progress** | `https://spot.calmdonut.com/` shows updated UX and `duration=180` returns 12 x 15min entries | M6 complete       |
| 2   | M7 closure: confirm Railway volume backup schedule evidence in docs                              | Agent | Ready           | Backup configuration evidence linked in gate artifact                                        | M7 complete       |
| 3   | M4 decision: finalize Better Auth session/signup scope for release                               | Owner | Ready           | Explicit GO (implement now) or defer decision documented                                     | Release readiness |

## Completed history

| Date       | Action                                  | Evidence                                                                    |
| ---------- | --------------------------------------- | --------------------------------------------------------------------------- |
| 2026-02-24 | Foundation research dossier approved    | `docs/research/2026-02-24-foundation-research.md` — owner GO                |
| 2026-02-24 | M0-M5 implementation slices merged      | `src/` (scaffolding, migrations, ingestion, calculator, API)                |
| 2026-02-24 | Production deploy smoke tests passed    | `https://spot.calmdonut.com/health` + `/api/v1/price/*` responses validated |
| 2026-02-24 | Gate evidence updated with prod checks  | `docs/quality-gates/2026-02-24-m0-m6-gate-check.md`                         |
| 2026-02-24 | Quarter-hour cheapest-window fix merged | `src/calculator.ts` + `src/calculator.test.ts`                              |
| 2026-02-24 | Dashboard UX hardening merged           | `src/ui.ts`                                                                 |

## Update cadence

- Update this file at every meaningful handoff and before claiming task completion.
- If active queue changes, update `docs/STATUS.md` in the same change.
