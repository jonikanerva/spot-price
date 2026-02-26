---
description: Create implementation plan from approved research dossier
agent: roadmap-planner
subtask: true
---

Use skill `rpi-plan`.
Input research artifact: $ARGUMENTS

## Branch Lifecycle

Before writing any output:

1. Verify you are on the initiative branch where the research dossier was committed (not `main`).
2. If on `main`, stop and ask — the research should have created the branch already.
3. After writing the plan, commit it: `git add docs/plans/... && git commit -m "docs(plan): add <topic> implementation plan"`.

Write output to:

- `docs/plans/YYYY-MM-DD-<topic>-plan.md`

Return:

1. Scope and objectives
2. Milestones and dependencies
3. Risks and mitigations
4. Acceptance criteria
5. Quality gates and signoff checklist
