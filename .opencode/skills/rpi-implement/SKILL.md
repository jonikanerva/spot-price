---
name: rpi-implement
description: Execute implementation only after research and plan gates are satisfied
---

## Objective

Implement approved plan steps with verification evidence.

## Preconditions

- Approved research document exists and is committed to the initiative branch.
- Approved plan document exists and is committed to the same branch.
- Quality gates are defined.
- You are on the initiative branch (not `main`). Do not create a new branch — research and plan are already here.

## Execution Rules

- Follow plan order unless a blocker is found.
- If deviation is required, log rationale and request plan update.
- Provide verification output for each completed task.

## Code Standards (mandatory)

- **Functional programming by default**: prefer pure functions, avoid side effects, maintain immutability. Use mutation only at I/O boundaries.
- **Strong types always**: never use `any`, `unknown` as bypass, or catch-all types. Every parameter and return value must have an explicit type.
- **No code duplication**: if a fix needs logic similar to existing code, refactor to reuse — do not copy-paste.

## Implementation Discipline (mandatory)

- **Do not skip repository checks**: run lint, format, type-check, and test commands before every commit — even for trivial edits.
- **Minimal scoped fixes**: change only what is necessary to fix the detected issue. Do not expand scope.
- **No unrelated refactors during fixes**: document improvement opportunities as follow-up tasks, do not mix into the current fix.
- **Holistic commits**: one logical change per commit. Separate commits for separate issues. Never mix unrelated fixes, features, or refactors.

## Output Contract

- Change summary
- Verification evidence
- Remaining risks or follow-ups
