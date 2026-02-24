# Decision Rights: Agent Command Autonomy

Date: 2026-02-22
Owner: Repository Owner + Agent
Status: Active draft

## Purpose

Define exactly which commands the agent can run autonomously and which require explicit human approval.

## Risk tiers

### Tier A: Auto-allow (no confirmation required)

Allowed by default when operating in the current local repository and feature branch:

- Read-only commands (`git status`, `git diff`, `git log`, file reads, search).
- Local quality checks (`test`, `lint`, `format`, static checks).
- Local non-destructive dev commands (build, benchmark, smoke runs).
- Feature branch operations (`git switch -c`, `git add`, `git commit`, `git push -u origin <feature-branch>`).
- Pull request creation from a feature branch.

### Tier B: Approval required (explicit user confirmation)

- Any write action outside local feature branch scope.
- Any edit to `docs/vision/VISION.md` (human approval required).
- Changes that touch secrets, credentials, auth, IAM, billing, or retention.
- Modifying protected branch settings or repository administration.
- External-system writes via integrations (bulk issue edits, automation that mutates remote state).
- Release-impacting actions (deploy, migration with production impact, data backfill against production).

### Tier C: Deny by default

- Destructive git/history commands (`git reset --hard`, `git push --force`, deleting remote branches without request).
- Commands intended to bypass governance safeguards (`--no-verify` on commit hooks).
- Unscoped mass deletion/overwrite operations.

Tier C actions are never run unless user explicitly requests the exact command and accepts the risk.

## Permission model

- Default principle: least privilege and deny-by-default for high-risk writes.
- Agent baseline scope: read + low-risk local write.
- Elevation to high-risk actions: one-time, explicit, auditable user approval.

## Safe examples

Auto-allow examples:

```bash
git status
git diff
swift test
git switch -c docs/agent-governance
git add docs/operating-model/decision-rights.md
git commit -m "docs(operating-model): define command decision rights"
git push -u origin docs/agent-governance
```

Approval-required examples:

```bash
git push origin main
git reset --hard HEAD~1
gh secret set OPENAI_API_KEY
./scripts/deploy-production.sh
```

## Enforcement notes

- `main` remains protected and PR-only.
- Pre-commit and CI checks remain mandatory.
- If risk is ambiguous, treat as Tier B and ask.
