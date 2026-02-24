# Definition of Done: Team + Agent

Date: 2026-02-22
Owner: Repository Owner + Agent
Status: Active draft

## Scope completion

- The implemented change matches approved scope from plan artifacts.
- Out-of-scope items are documented as follow-up tasks, not silently included.

## Quality and verification

- Relevant formatter/lint/test/smoke checks pass for touched scope.
- **No check skipping**: run required lint, format, type-check, and test commands before every commit — even for small or "trivial" edits.
- If tests are missing, a reproducible verification command is documented.
- Evidence for claims is included (logs, artifacts, or links).

## Code quality standards

- **Functional programming by default**: prefer pure functions, avoid side effects, maintain immutability.
- **Strong types always**: no `any`, `unknown` as bypass, or similar catch-all types. Every parameter and return value has an explicit type.
- **No code duplication**: refactor to reuse existing logic instead of copying. DRY applies to logic, not just strings.

## Git and review readiness

- Work is on a non-protected branch.
- Commits are single-purpose, readable, and follow commit convention.
- **Holistic commits**: each commit contains exactly one logical change — no mixed fixes, features, or refactors.
- PR description includes verification steps, risks, and assumptions.

## Safety and compliance

- No secrets, credentials, or sensitive data are committed.
- Risky operations followed decision-rights policy and approval boundaries.
- Rollback path is defined (`git revert` or equivalent safe fallback).

## Documentation and handoff

- Relevant docs under `docs/` are updated when behavior/process changed.
- `docs/STATUS.md` reflects the current phase, active objective, and next actions.
- Handoff message states what changed, what was verified, and what remains.
- Reviewer can reproduce validation using documented commands.

## Gate alignment

- If quality-gate criteria exist, gate status is recorded or explicitly marked pending.
- No release-ready claim is made without required gate evidence and signoff.
