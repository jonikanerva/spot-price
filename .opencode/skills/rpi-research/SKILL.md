---
name: rpi-research
description: Produce an evidence-backed research dossier as the first RPI phase
---

## Objective

Create a research artifact that is decision-ready for planning.

## Required Sections

- Problem statement
- Target users and jobs-to-be-done
- Constraints (business, technical, regulatory)
- Assumptions and unknowns
- Alternatives with tradeoffs
- Evidence and source links
- Recommendation

## Branch Lifecycle

- Before writing any output, verify you are on an initiative branch (not `main`).
- If no branch exists yet, create one: `git switch -c feat/<topic>` from `main`.
- After writing the research dossier, commit it: `docs(research): add <topic> research dossier`.
- The plan and implementation phases will commit to the same branch.

## Rules

- Cite sources for all non-trivial claims.
- Explicitly label assumptions vs facts.
- Do not produce implementation steps.
