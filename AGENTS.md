# Agentic Product Delivery Rules

## Operating Default

- Enforce RPI workflow: **Research -> Plan -> Implement** for every meaningful change.
- Research is mandatory before planning; planning is mandatory before implementation.
- Prefer custom commands (`/research`, `/plan`, `/implement`, `/gate-check`) over ad-hoc prompts for repeatability.
- Default interaction should use the RPI orchestrator, which auto-dispatches specialist agents when prompt context matches their domain.
- At session start, read `docs/STATUS.md` first, then read the full `docs/operating-model/` folder before implementation.

## Product-to-Delivery Flow

1. Research dossier drafted and challenged (problem, users, constraints, alternatives, evidence).
2. Plan generated from approved research with dependencies, risks, and acceptance criteria.
3. Quality gates defined against the plan before implementation starts.
4. Implementation executes only after gate pass and owner signoff.

## Quality Expectations

- All claims must include evidence or source links.
- Risks, assumptions, and unknowns must be explicit.
- Changes to priorities require updated roadmap rationale.

## Code Standards

- **Functional programming by default**: prefer pure functions, avoid side effects, maintain immutability. Use mutation only when explicitly justified (e.g., database writes, I/O boundaries).
- **Strong types always**: never use `any`, `unknown` as a bypass, or similar catch-all types. Every function parameter and return value must have an explicit type. Type safety is non-negotiable.
- **No code duplication**: if a fix or feature requires logic similar to existing code, refactor to reuse — do not copy-paste. DRY applies to logic, not just strings.

## Implementation Discipline

- **Do not skip repository checks**: run required lint, format, type-check, and test commands before every commit, even for small or "trivial" edits. No exceptions.
- **Minimal scoped fixes**: when resolving a check failure or bug, change only what is necessary to fix the detected issue. Do not expand scope.
- **No unrelated refactors during fixes**: if you notice improvement opportunities while fixing a bug, document them as follow-up tasks — do not mix them into the fix.
- **Holistic commits**: each commit contains exactly one logical change. If multiple issues are fixed, create separate commits for each. Never mix unrelated fixes, features, or refactors in a single commit.

## Required Artifacts

- Research outputs live under `docs/research/`.
- Plans live under `docs/plans/`.
- Gate results live under `docs/quality-gates/`.
