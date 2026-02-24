---
description: Execute implementation only from an approved plan
agent: build
subtask: false
---

Use skill `rpi-implement`.
Input approved plan artifact: $ARGUMENTS

Execution constraints:
- Refuse to proceed if plan is missing or not approved.
- Implement tasks in order and keep changes minimal.
- Report verification evidence for each completed task.

Write implementation notes to:
- `docs/plans/YYYY-MM-DD-<topic>-execution-log.md`
