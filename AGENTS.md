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

## Required Artifacts
- Research outputs live under `docs/research/`.
- Plans live under `docs/plans/`.
- Gate results live under `docs/quality-gates/`.
