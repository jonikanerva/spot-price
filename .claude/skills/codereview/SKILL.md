---
name: codereview
description: >
  Full code review loop. Reviews PR, posts findings as audit trail,
  fixes issues, and re-reviews until passing or escalating to human.
  Use after creating a PR or when re-reviewing after changes.
allowed-tools: Read, Write, Edit, Grep, Glob, Bash, Agent
argument-hint: [pr-number]
---

# Code Review Workflow

Automated review loop for pull requests. Reviews the PR, posts findings
as an audit trail comment, fixes issues if found, and re-reviews until
passing or escalating to human.

Communicate with the user in Finnish. All PR comments and review text
in English.

## Procedure

Follow these steps in order. Do not skip steps.

### Step 0: Identify the PR

If `$ARGUMENTS` contains a PR number, use that. Otherwise detect it:

```
gh pr list --head $(git branch --show-current) --json number --jq '.[0].number'
```

If no open PR is found, inform the user and stop.

Store the PR number for use throughout this workflow.

### Step 1: Activate Node.js

```
source ~/.nvm/nvm.sh && nvm use
```

### Step 2: Launch review subagent

Use the Agent tool to spawn an isolated reviewer. The subagent must
be read-only. Pass this exact prompt to the Agent tool:

> You are a code reviewer. Run these commands for PR #NUMBER:
>
> 1. `gh pr view NUMBER`
> 2. `gh pr diff NUMBER`
> 3. `gh pr checks NUMBER`
> 4. `gh pr view NUMBER --comments`
>
> Review criteria:
>
> - **Scope verification**: does the diff match the PR description?
>   Flag any undocumented changes — especially removals, renames, or
>   architectural shifts. If the PR description is missing or vague,
>   FAIL.
> - **Code quality**: no `any` types, pure functions, DRY, explicit
>   error handling, strong TypeScript
> - **Security**: no secrets, parameterized SQL, input validation
> - **Architecture**: separation of concerns, consistent patterns
> - **Commits**: one logical change per commit, clear messages
> - **Language**: all artifacts in English
> - **Tests**: new logic has tests, no broken tests
>
> **CI checks must be green** (`gh pr checks`) before verdict can be
> PASS.
>
> Verdict: **PASS** (no critical or important issues, CI green) or
> **FAIL**.
>
> Structure the review as:
>
> - 1-2 sentence summary of what the PR does
> - Findings by severity: critical → important → minor
> - Each finding: `file:line` — what is wrong and why it matters
> - Clear verdict: PASS or FAIL
>
> Return the complete review text and the verdict.

Replace NUMBER with the actual PR number in the prompt.

### Step 3: Post review comment — REQUIRED, NEVER SKIP

Take the review text from the subagent and post it as a PR comment.
This is the audit trail and must never be skipped regardless of
verdict.

```
gh pr comment NUMBER --body "<full review text>"
```

### Step 4: Attempt formal review action

This may fail on own PRs — that is expected. The comment is what
matters.

- For PASS: `gh pr review NUMBER --approve --body "See review comment above"`
- For FAIL: `gh pr review NUMBER --request-changes --body "See review comment above"`

If the command fails, note it and continue. Do not treat as error.

### Step 5: Handle verdict

**If PASS**: inform the user (in Finnish) that the review passed.
Do NOT merge the PR. Done.

**If FAIL**: proceed to Step 6.

### Step 6: Fix issues (FAIL path)

Track the current review iteration. Maximum **3 iterations** of the
fix-review loop.

For each finding marked critical or important:

1. Fix the issue in the code
2. Stage and commit the fix as a **separate commit** with a clear
   message explaining what was fixed and why
3. After all fixes are committed, push:
   ```
   git push origin <branch-name>
   ```
4. Post a PR comment explaining each fix:
   ```
   gh pr comment NUMBER --body "<explanation of fixes made>"
   ```

Do not bundle multiple unrelated fixes into one commit.

### Step 7: Re-run verification

```
pnpm test:all
```

All must pass before re-review. If failing, fix and re-run (max 5
attempts). If still failing, stop and ask the user.

### Step 8: Wait for CI

After pushing fixes, check CI status:

```
gh pr checks NUMBER
```

If checks are still running, wait briefly and re-check. If checks
fail, analyze the failure, fix, and push before re-reviewing.

### Step 9: Re-review (loop)

Go back to **Step 2** with a fresh subagent. Increment the iteration
counter.

If this is iteration 3 and the review still fails, **do not loop
again**. Instead:

1. Post a PR comment summarizing remaining issues
2. Inform the user (in Finnish) that the review could not resolve all
   issues after 3 attempts
3. List the remaining findings
4. Ask the user for guidance

## Rules

- **ALWAYS** post review findings as a PR comment — non-negotiable
- **NEVER** merge the PR, even if review passes
- **NEVER** skip the subagent — reviews must come from isolated context
- **NEVER** force push
- Each fix must be a separate commit
- Each fix round must have a corresponding PR comment
- Maximum 3 review iterations before escalating to human
