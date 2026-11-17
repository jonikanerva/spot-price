# 0009. The database moves from Railway PostgreSQL to Neon

- Status: accepted
- Date: 2026-11-16

## Context

The owner moves the databases of the projects on the Railway Hobby plan to
Neon to lower the monthly cost. mastofeed and caltodo moved on 2026-11-15.

The spot-price database differs from those two databases:

- It holds 467 MB (2026-11-15) and grows about 5 MB a day. The Fingrid
  forecast vintages are the largest table. The Neon Free plan gives 1 GB
  of storage a project.
- The pool kept 3 connections open, so the first request after an idle
  period did not open a connection. On Neon, open connections keep the
  compute active. An always-active compute uses about 182 CU-hours a
  month. The Free plan gives 100 CU-hours.
- The Fingrid forecast vintages and the weather forecasts cannot be
  fetched again. The account, the contract settings, and the API key exist
  only in the database.

Issue #188 records the facts, the procedure, and the evidence.

## Decision

1. The service uses the Neon project `spot-price`
   (`autumn-dream-98820540`, `aws-eu-central-1`, PostgreSQL 18) on the
   Free plan. The service stays on Railway.
2. The service connects to the direct Neon endpoint with
   `sslmode=verify-full`. It does not use the Neon pooler.
3. The pool keeps no idle connection open (`min_connections` 0), so the
   compute suspends after 5 minutes without queries.
4. D1 (owner, 2026-11-16): the retention does not change. The archive of
   datasets 166 and 248 continues. The owner changes to a paid Neon plan
   before the database reaches 850 MB (#168).
5. D2 (owner, 2026-11-16): the owner accepts the latency of the first
   request after a compute suspend. API requests have no latency budget
   (`STACK.md` §4).
6. D3 (owner, 2026-11-16): Neon becomes the hosting recipient of the
   database, including the account, the hashed password, the sessions, the
   contract settings, and the plaintext API key (`VISION.md`).
7. A fresh copy (`pg_dump` and `pg_restore`) moves the data right before
   the switch of `DATABASE_URL`, between the hourly jobs. The checksum of
   every table must match. After Railway removes the previous deployment,
   the source checksums must still match, so no write is lost.
8. The Railway service `Postgres spot-price` stays unchanged as the
   rollback database. The owner decides when to remove it. Update of
   2026-11-17: the owner removed the service and its volume, after a
   `pg_dump` of the Neon database. No service referred to the Railway
   database, and it held the data of 2026-11-16 11:17 UTC only.

## Alternatives

- **Stay on Railway PostgreSQL.** No new recipient and no extra latency.
  It keeps the Railway cost of the database service.
- **Move and shorten the vintage retention** (180 days without datasets
  166 and 248, or 75 days for all datasets). The database then stays under
  1 GB for about three years. Rejected by the owner: keep the archive and
  the retention (D1).
- **Paid plan with an always-active compute and `min_connections` 3.**
  Rejected: about 182 CU-hours a month for a latency that the owner does
  not need.
- **Neon pooler endpoint.** Rejected: the SQLx migrator holds a session
  advisory lock, and the pool sends `statement_timeout` as a startup
  parameter. PgBouncer in transaction mode breaks both.

## Consequences

- Database traffic goes over the public internet from `europe-west4` to
  Frankfurt, with TLS and a certificate check. The first request after a
  compute suspend waits for the compute start and a new TLS connection.
- When the project uses all CU-hours of a month, Neon suspends the compute
  until the next month, and every database request fails. The estimate is
  about 43 CU-hours a month (`STACK.md` §5).
- At 1 GB on the Free plan, writes fail: the price job stores no new
  prices. The owner must change the plan first. Neon does not upgrade the
  plan automatically.
- The Neon history window of 6 hours adds a short restore path that
  Railway Hobby did not have.
- The cost moves from the Railway database service to Neon only after the
  owner removes `Postgres spot-price` (done on 2026-11-17). A paid Neon plan adds a monthly cost
  for storage above the Free plan.

## Cost of reversal

Medium. The owner removed `Postgres spot-price` on 2026-11-17, so a
reversal is a data move to a new database (`STACK.md` §5 Data move), then a
change of `DATABASE_URL`. No code change is necessary.

## Reassessment condition

Reassess when the database reaches 850 MB, when the Neon project uses more
than 80 CU-hours in a month, when the Neon Free plan terms change, or when a
cold start makes a request fail.
