# 0008. Local PostgreSQL runs in Apple containers, with one shared script

- Status: accepted
- Date: 2026-11-15

## Context

The tests, the SQLx checks, and the e2e journeys used one native PostgreSQL
server per checkout on the fixed port 55436. Agents work in many worktrees
at the same time, so the runs collided. PR #183 moved every local server
into Apple `container`: one dev server per worktree and one test server per
run.

caltodo and mastofeed made the same move at the same time, each with its
own script. The three scripts differed in names, task names, passwords,
version pins, signal handling, and clean-up. The owner wants one behaviour
in the three repositories. The owner also wants the database to be
invisible: `dev` and the test tasks start what they need, and old servers go
away without a manual step.

## Decision

1. Every local PostgreSQL server runs in an Apple `container` of the
   official `postgres` image. `SPOT_PRICE_PG_IMAGE` in `mise.toml` pins the
   tag and the digest.
2. `scripts/db.sh` is byte-identical in caltodo, mastofeed, and spot-price.
   `scripts/db-project.sh` holds the differences of each project: names,
   database names, the port range, TLS, and the project commands.
3. Each worktree has one dev server with a volume named by the major
   version. The commands that need it start it; the last one stops it at
   exit. `db:start` keeps it running until `db:stop`. A stop removes the
   container and keeps the volume: a stopped container keeps a copy of the
   image (about 750 MB). Added on 2026-11-15 after the owner saw the disk
   use of stopped containers.
4. Each independent test run gets a new test server on tmpfs with fsync
   off. `verify` shares one server over its steps.
5. Every command that starts a server first removes the test servers of
   ended runs and the dev server and dev volume of removed worktrees. A
   worktree counts as removed only when git no longer lists it, its
   directory is gone, and its parent directory exists.
6. `db:restore`, `db:clone`, and `db:reset` replace the dev database and
   first write a non-empty database to `.tmp/db/backups/`.
7. `.claude/settings.json` lists the database tasks in
   `sandbox.excludedCommands`, because the `container` tool needs XPC.

## Alternatives

- Keep three scripts and align only their interfaces: less work now, but
  the scripts drift again with each fix.
- A shared script in the agent-setup repository: one source, but the owner
  keeps agent-setup free of project test tooling, and a copy step would
  still be needed.
- Keep the dev data of removed worktrees until a manual clean-up: no data
  loss after a moved worktree, but the volumes pile up. Agent worktrees hold
  disposable data.
- A limit of test servers over all repositories of the Mac: the owner
  decided against it (2026-11-15). Each repository keeps its own limit of 4.

## Consequences

- A developer or an agent runs only `dev`, `test`, `e2e`, `sqlx:*`, or
  `verify`. The `db:` tasks are optional tools.
- A moved worktree loses its dev data. Use `db:dump` before the move.
- A fix to `scripts/db.sh` goes to all three repositories. Compare the files
  with `diff` or `shasum`.
- The sandbox entries match only a plain call such as `mise run verify`.

## Cost of reversal

Low. Restore the earlier `scripts/db.sh` from git. No production data
depends on this decision.

## Reassessment

When the container tool changes its CLI or networking in a way that breaks
`scripts/db.sh`, or when the owner leaves the Apple silicon Mac.
