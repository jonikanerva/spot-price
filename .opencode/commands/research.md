---
description: Create a research dossier before planning
agent: research-analyst
subtask: true
---

Use skill `rpi-research`.
Research topic: $ARGUMENTS

## Branch Lifecycle

Before writing any output:

1. If already on an initiative branch (not `main`): continue on it.
2. If on `main`: create a new branch first — `git switch -c feat/<topic>`.
3. After writing the dossier, commit it: `git add docs/research/... && git commit -m "docs(research): add <topic> research dossier"`.

Write output to:

- `docs/research/YYYY-MM-DD-<topic>.md`

Return:

1. Problem framing
2. User/jobs context
3. Constraints and assumptions
4. Alternatives and tradeoffs
5. Evidence with source links
6. Unknowns and risk flags
