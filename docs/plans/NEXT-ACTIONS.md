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

| #   | Action                                                                                | Owner | Status          | Acceptance check                                                 | Target               |
| --- | ------------------------------------------------------------------------------------- | ----- | --------------- | ---------------------------------------------------------------- | -------------------- |
| 1   | M6 hardening: polish dashboard UX (settings editor, error states, mobile refinements) | Agent | **In progress** | UI supports key flow + settings + today/cheapest views on mobile | M6 complete          |
| 2   | M7 setup: configure Railway deployment + volume + env vars + health checks            | Agent | Ready           | Service boots on Railway and `/health` reports DB status         | M7 setup complete    |
| 3   | Gate evidence: record quality gate result for current implementation slice            | Agent | Ready           | Gate artifact exists under `docs/quality-gates/`                 | Before release claim |

## Completed history

| Date       | Action                               | Evidence                                                     |
| ---------- | ------------------------------------ | ------------------------------------------------------------ |
| 2026-02-24 | Foundation research dossier approved | `docs/research/2026-02-24-foundation-research.md` — owner GO |
| 2026-02-24 | M0-M5 implementation slices merged   | `src/` (scaffolding, migrations, ingestion, calculator, API) |

## Update cadence

- Update this file at every meaningful handoff and before claiming task completion.
- If active queue changes, update `docs/STATUS.md` in the same change.
