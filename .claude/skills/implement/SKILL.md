---
name: implement
description: >
  Full implement-and-ship workflow. Use when asked to implement a feature,
  fix a bug, or make any code change that should be shipped as a PR.
  Does not include planning — use /plan mode first for non-trivial work.
allowed-tools: Read, Write, Edit, Grep, Glob, Bash, Agent
argument-hint: <description of what to implement>
---

# Implement and Ship Workflow

Complete workflow for implementing a change and shipping it as a PR.
The task description comes from `$ARGUMENTS`.

Communicate in Finnish with the user. All code artifacts (commits,
branch names, PR text, code comments) in English per project policy.

## Procedure

Follow these steps in order. Do not skip steps.

### Step 1: Activate Node.js

```
source ~/.nvm/nvm.sh && nvm use
```

### Step 2: Ensure feature branch

Check the current branch:

```
git branch --show-current
```

- If on `main`: create and switch to a feature branch. Derive the
  branch name from the task. Use `feat/<slug>`, `fix/<slug>`, or
  `chore/<slug>`. Keep under 50 characters, lowercase, hyphens only.
- If already on a feature branch: stay on it. Run
  `git log main..HEAD --oneline` to understand the current state.

**NEVER commit or push to `main` directly.**

### Step 3: Implement the change

Implement what is described in `$ARGUMENTS`. Follow all project
standards from CLAUDE.md:

- Strong TypeScript: no `any`, no `unknown` as bypass
- Pure functions, side effects only at I/O boundaries
- DRY: reuse existing code, never copy-paste
- Single-purpose functions
- Descriptive English names
- UTC everywhere internally
- Every new feature or behavior change must have tests

If the task is unclear or ambiguous, ask the user for clarification
before writing code.

### Step 4: Run verification

```
pnpm test:all
```

This runs: typecheck → lint → test → build. **All must pass.**

If verification fails:

1. Read the error output carefully
2. Fix the issue
3. Re-run `pnpm test:all`
4. Repeat until all checks pass
5. Maximum 5 fix attempts — if still failing, stop and ask the user

### Step 5: Commit

Stage only the files related to this change. Never use `git add -A`
or `git add .` blindly. Review what is being staged.

Never commit `.env` files, credentials, or secrets.

Write a commit message that:

- Is concise (1-2 sentences)
- Focuses on "why" not "what"
- Is in English

Each commit must be one complete logical unit. If multiple logical
changes were made, create separate commits for each.

### Step 6: Push

```
git push -u origin <branch-name>
```

### Step 7: Create or update PR

Check if a PR already exists for this branch:

```
gh pr list --head <branch-name> --json number,url --jq '.[0]'
```

**If no PR exists**, create one:

```
gh pr create --title "<concise title>" --body "<description>"
```

The PR body must describe what is being changed and why. Note any
design decisions or trade-offs. Keep the title under 70 characters.

**If a PR already exists**, add a comment describing what changed:

```
gh pr comment <number> --body "<what changed and why>"
```

### Step 8: Report to user

Tell the user (in Finnish):

- Summary of what was implemented
- Verification results (all passing)
- PR URL
- Suggest: "Aja `/codereview` kun olet valmis reviewiin."

## Rules

- **NEVER** force push
- **NEVER** push to main
- **NEVER** add dependencies without explicit user approval
- **NEVER** commit secrets or credentials
- **NEVER** merge the PR — that happens after review
- If `pnpm test:all` does not pass, do NOT push or create PR
