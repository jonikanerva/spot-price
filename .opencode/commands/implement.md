---
description: Execute implementation only from an approved plan
agent: build
subtask: false
---

Use skill `rpi-implement`.
Input approved plan artifact: $ARGUMENTS

## Branch Lifecycle

Before starting implementation:

1. Verify you are on the initiative branch where research and plan are already committed (not `main`).
2. If on `main`, stop and ask — the research and plan phases should have created and committed to the branch already.
3. Do not create a new branch or worktree — the initiative branch is your workspace.

Execution constraints:

- Refuse to proceed if plan is missing or not approved.
- Implement tasks in order and keep changes minimal.
- Report verification evidence for each completed task.

Write implementation notes to:

- `docs/plans/YYYY-MM-DD-<topic>-execution-log.md`
