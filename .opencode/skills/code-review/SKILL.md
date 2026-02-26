---
name: code-review
description: Review PR changes for code quality, security, architecture, and project standards before merge
---

## Purpose

Comprehensive code review for pull requests. Enforces project code standards, security practices, architectural consistency, and the language policy. When review passes, auto-merges the PR and cleans up branches.

## When to Use

- Before merging any pull request to `main`
- After completing a feature branch implementation
- As the final gate in the RPI workflow (after implementation verification)

## Review Scope

### 1. Code Quality

- **Functional programming**: pure functions preferred, side effects only at I/O boundaries
- **Strong types**: no `any`, `unknown` as bypass, or catch-all types — every parameter and return value explicitly typed
- **No duplication**: DRY applied to logic, not just strings — refactor to reuse
- **Single-purpose functions**: each function does one thing well
- **Error handling**: explicit, no swallowed errors, consistent error format
- **Naming**: descriptive, intention-revealing names in English

### 2. Security

- No secrets, credentials, API keys, or tokens in code or commits
- No hardcoded sensitive values (check for patterns: passwords, tokens, private keys)
- Input validation on all external boundaries (API endpoints, user input)
- SQL injection prevention (parameterized queries only)
- Authentication/authorization checks on protected routes
- Rate limiting on abuse-prone endpoints
- Dependency audit: no known vulnerabilities in new dependencies

### 3. Architecture and Modularity

- Clear separation of concerns (routes, business logic, data access, UI)
- No circular dependencies between modules
- New code fits existing architectural patterns (check `src/` structure)
- Database access isolated from business logic
- Configuration externalized (environment variables, not hardcoded)
- Consistent module boundaries — no god files

### 4. Project Standards (from AGENTS.md)

- **Holistic commits**: one logical change per commit, no mixed concerns
- **Minimal scoped changes**: no scope creep beyond the PR intent
- **No unrelated refactors**: improvement opportunities documented as follow-ups, not mixed in
- **Repository checks pass**: typecheck, lint, format, tests, build

### 5. Language Policy

- All code, comments, commit messages, branch names, variable names, error messages, and documentation in English
- No non-English strings in source code except user-facing Finnish content explicitly required by product spec

### 6. Test Coverage

- New logic has corresponding tests
- Tests verify behavior, not implementation details
- Edge cases covered (null, empty, boundary values, error paths)
- No test-only code paths in production code
- Existing tests not broken by changes

## Review Process

```
1. Read the PR diff (git diff main...HEAD or base...head)
2. Read the PR description and linked plan/research if referenced
3. Run verification commands:
   - npm run typecheck
   - npm run lint
   - npm run format:check
   - npm test
   - npm run build
4. Evaluate against all 6 review categories above
5. Categorize findings by severity
6. Render verdict
```

## Output Format

```markdown
## Code Review: [PR title]

### Verification

- typecheck: PASS/FAIL
- lint: PASS/FAIL
- format: PASS/FAIL
- tests: PASS/FAIL (N tests)
- build: PASS/FAIL

### Strengths

[Specific positive observations with file:line references]

### Issues

#### Critical (Must Fix)

[Bugs, security vulnerabilities, data loss risks, broken functionality]

#### Important (Should Fix)

[Architecture violations, missing validation, type safety gaps, test gaps]

#### Minor (Nice to Have)

[Style improvements, documentation, optimization opportunities]

**Each issue includes:**

- File:line reference
- What is wrong
- Why it matters
- How to fix

### Verdict

**LGTM** — ready to merge, no blocking issues found.

OR

**Changes Requested** — [N] critical, [N] important issues must be resolved.
[List of required fixes before re-review]
```

## Post-Review Actions

### On LGTM verdict:

1. Post a summary comment on the PR with the review result
2. Merge the PR to main: `gh pr merge <number> --merge --delete-branch`
3. Clean up local branch: `git switch main && git pull && git branch -d <branch>`
4. Prune stale worktrees: `git worktree prune`

### On Changes Requested verdict:

1. Post the review findings as a PR comment
2. List exact fixes required
3. Implementation agent applies fixes and re-requests review
4. Re-review only the changed files plus any files affected by fixes

## Critical Rules

- Never approve code with `any` types
- Never approve code with duplicated logic
- Never approve code with secrets or credentials
- Never approve code that breaks existing tests
- Never approve non-English artifacts (code, comments, commits, docs)
- Always run verification commands — never trust "it should work"
- Be specific: file:line references, not vague observations
- Acknowledge strengths — reviews are not just about problems
