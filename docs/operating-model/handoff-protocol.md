# Handoff Protocol: Commit Progression and Escalation

Date: 2026-02-22
Owner: Repository Owner + Agent
Status: Active draft

## Purpose

Define how work progresses from local edits to review-ready commits with minimal interruption and predictable handoffs.

## Workspace isolation default

- Default mode: create one `git worktree` per implementation session.
- One session = one worktree = one task branch = one PR scope.
- Goal: keep `index` and working tree isolated between parallel agents/sessions.

Allowed exceptions (must be documented in PR or handoff note):

- Short single-developer hotfix with no parallel agent activity.
- Read-only analysis/documentation session with no commits.
- Submodule or repo edge case where worktree is known to be unstable (use separate clone).

## Branch and commit progression

1. Start from updated base and create a session worktree + task branch.
   - Naming: `feat/<topic>`, `fix/<topic>`, `docs/<topic>`, `chore/<topic>`.
2. Ensure GitHub App auth + bot identity are configured in the active worktree (`./scripts/use-github-app-auth.sh`).
3. Implement one logical change at a time.
4. Run local verification for changed scope.
5. Create single-purpose commit with clear message.
6. Repeat until task scope is complete.
7. Push branch and open PR.
8. Merge to `main` only via reviewed PR.

Worktree creation example:

```bash
mkdir -p .worktrees
git worktree add ".worktrees/<topic>-<session>" -b "feat/<topic>" main
```

## Commit cadence

- Commit at logical milestones, usually every 30-90 minutes of focused work.
- Avoid mixed-purpose commits.
- Keep commit history reviewable: why-first, what-second.

## Commit message convention

Use Conventional Commit style:

```text
<type>(<scope>): <imperative summary>
```

Examples:

- `docs(operating-model): define agent decision-right tiers`
- `chore(ci): enforce gate-check artifact validation`

## Pre-commit verification minimum

Before each commit, run the smallest meaningful checks:

- Formatter/linter for touched files.
- Targeted tests for impacted behavior.
- If no tests exist, run relevant smoke command.

If checks fail:

- Do not commit until fixed, or
- Commit only if explicitly agreed and message explains temporary failure context.

## Escalation triggers (ask user)

Agent pauses and asks for explicit confirmation when:

- Requested command is Tier B or Tier C from decision rights.
- Action would modify protected branch or shared remote history.
- Action impacts production/runtime data outside local environment.
- Security-sensitive data might be exposed or committed.

## Rollback defaults

- Preferred rollback: `git revert <sha>` for already committed branch changes.
- For uncommitted local mistakes: safe targeted edits (avoid destructive reset unless explicitly requested).
- Never force-push `main`; avoid force push on any branch unless explicitly requested.

## Merge and cleanup cadence

- After PR merge, remove the session worktree within 24h.
- Run `git worktree prune` at least weekly.
- Keep stale session branches out of local workspace; delete merged branches during cleanup.

Cleanup example:

```bash
git worktree remove ".worktrees/<topic>-<session>"
git branch -d feat/<topic>
git worktree prune
```

## PR handoff checklist

- [ ] Branch is pushed and tracks remote.
- [ ] Commits are single-purpose and readable.
- [ ] Verification commands and outcomes are recorded in PR description.
- [ ] Risks/assumptions are stated.
- [ ] Reviewer can reproduce validation steps.
- [ ] `docs/STATUS.md` and `docs/plans/NEXT-ACTIONS.md` reflect the latest state.
