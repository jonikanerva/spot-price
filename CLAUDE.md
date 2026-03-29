# CLAUDE.md — spot-price

## Project Overview

Nord Pool spot electricity price API with total price calculation and cheapest window finder.

- **Runtime**: Node.js 24 LTS, TypeScript (strict mode)
- **Framework**: Hono + @hono/node-server
- **Database**: SQLite via better-sqlite3 (WAL mode)
- **Auth**: Better Auth (self-hosted)
- **Package manager**: pnpm
- **Tests**: Vitest (unit) + Playwright (E2E)
- **Build**: tsup
- **Hosting**: Railway

## Language Policy

- All project artifacts in **English**: code, comments, commits, branch names, PR titles, variable names, error messages.
- User communication in **Finnish**.

## Verification

Node.js and pnpm are managed via nvm. Always activate first: `source ~/.nvm/nvm.sh && nvm use`

Run before every commit and PR — all must pass, no exceptions:

```
pnpm test:all
```

This runs: `typecheck → lint → test → build`

## Code Standards

- **Strong TypeScript**: no `any`, no `unknown` as bypass. Every parameter and return value explicitly typed.
- **Functional programming**: pure functions preferred, side effects only at I/O boundaries (database, HTTP, file I/O).
- **DRY**: if logic is similar to existing code, refactor to reuse. Never copy-paste.
- **Single-purpose functions**: each function does one thing.
- **Naming**: descriptive, intention-revealing, English.
- **UTC everywhere**: all internal code, database storage, and logs use UTC timestamps. Timezone conversion happens only at the edge — inbound requests convert to UTC immediately, outbound responses/UI convert at the last moment.

## Git Workflow

- Every feature gets its own branch. Branch from `main`, PR back to `main`.
- **NEVER** commit or push directly to `main`.
- **NEVER** force push (`--force` or `--force-with-lease`).
- Commits must be complete logical units — one logical change per commit.
- Commit messages: concise, English, focus on "why" not "what".
- PRs are merged with **merge commit** (`gh pr merge --merge --delete-branch`), not squash. Always delete the branch after merge.
- **PR as audit trail**: the PR description must fully describe what is being changed and why. Every correction after a failed review must be a separate commit + push + PR comment explaining the fix. Design decisions, trade-offs, and compromises must be documented in PR comments — the PR is the permanent record.

## Dependencies

**NEVER** add a new dependency without research and explicit human approval.

When proposing a dependency, provide:

- Name and purpose
- Bundle size impact
- Maintenance status (last release, open issues)
- Alternatives considered and why this one wins

## Safeguards

- **NEVER** read `.env` files (`.env`, `.env.*`, `.env.local`, `.env.production`).
- **NEVER** commit secrets, credentials, API keys, or tokens.
- **NEVER** run `rm -rf` on project directories.
- **NEVER** merge a PR without all verification passing.

## Planning

Use Claude Code's built-in `/plan` mode for any non-trivial work. Before implementation, research:

- Modern TypeScript patterns relevant to the change
- Existing project patterns that should be followed
- Architecture impact

## Code Review

When a PR is ready for review, launch a **separate review subagent** using the Agent tool with this prompt:

> You are a code reviewer. Run these commands for PR #NUMBER:
>
> 1. `gh pr view NUMBER`
> 2. `gh pr diff NUMBER`
> 3. `gh pr checks NUMBER`
> 4. `gh pr view NUMBER --comments`
>
> Review criteria:
>
> - **Scope verification**: does the diff match the PR description? Flag any undocumented changes — especially removals, renames, or architectural shifts. If the PR description is missing or vague, FAIL.
> - **Code quality**: no `any` types, pure functions, DRY, explicit error handling
> - **Security**: no secrets, parameterized SQL, input validation
> - **Architecture**: separation of concerns, consistent patterns
> - **Commits**: one logical change per commit, clear messages
> - **Language**: all artifacts in English
> - **Tests**: new logic has tests, no broken tests
>
> **CI checks must be green** (`gh pr checks`) before verdict can be PASS.
>
> Verdict: **PASS** (no critical/important issues, CI green) or **FAIL**.
>
> **Always** post the full review as a PR comment (`gh pr comment --body [review]`) — this is the audit trail and must never be skipped.
>
> Then attempt the formal review action (may fail on own PRs — that's OK, the comment is what matters):
> - For FAIL: `gh pr review --request-changes --body "See review comment above"`
> - For PASS: `gh pr review --approve --body "See review comment above"`. Do NOT merge.
>
> Review comment structure:
>
> - Start with a 1-2 sentence summary of what the PR does
> - List findings by severity: critical → important → minor
> - Each finding: `file:line` — what is wrong and why it matters
> - End with a clear verdict: PASS or FAIL

## Testing

- Every new feature or behavior change must have tests. No exceptions.
- Edge cases must be explicitly tested — not just the happy path.
- `src/api-schemas.ts` is the single source of truth for the public API contract and OpenAPI documentation. Response schema conformance tests must exist and pass — API responses must never diverge from the declared schemas.
- Existing tests must not be deleted or weakened without justification.

## Implementation Discipline

- Minimal scoped fixes: change only what is necessary.
- No unrelated refactors during fixes — document them as follow-ups.

## Key Paths

- Deployment: `RAILWAY.md` — production runs on Railway, use `railway` CLI for logs/status queries
- Smoke test: `pnpm smoke` (runs `scripts/smoke-local.sh`)
- E2E tests: `pnpm test:e2e`
