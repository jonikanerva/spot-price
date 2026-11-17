# 0010. Contract terms are versioned by local date; the TypeScript revert path ends

- Status: accepted
- Date: 2026-11-17

## Context

`user_settings` held one row of contract terms for each user. Every price
route applied that row to every interval. The margin of the owner's
contract changes on 2026-12-01. With one row:

- `/api/v1/price/history` computes old days with the new terms.
- On the evening before a change, `/api/v1/price/tomorrow`,
  `/api/v1/price/cheapest`, and the forecast price the next day with the old
  terms.

The owner asked for versioned settings and approved a `valid_from` model
(2026-11-17). The same day the owner ended the revert path to the TypeScript
service (ADR 0001).

## Decision

1. The contract terms are versioned: margin, day transfer, night transfer,
   tax, VAT, and the night hours. The time zone and the area stay single
   current values in `user_settings`.
2. The table `contract_terms` holds one row for each version, with the
   primary key `(user_id, valid_from)`. `valid_from` is a local calendar
   date (`DATE`) in the time zone of the user.
3. An interval uses the version of the local date of the interval start:
   the latest version with `valid_from` on or before that date. An interval
   before the first version uses the first version. Every interval has a
   version.
4. Migration 4 copies each `user_settings` row as the version of
   `1970-01-01`. The terms of today thus apply from the start, which the
   owner confirmed as correct.
5. `PUT /api/v1/me/settings` accepts `validFrom`. The default is today in
   the time zone of the user. A save for an existing date replaces that
   version. A save of the terms that already apply on the date creates no
   version. A user holds 1 to 100 versions. The setup UI lists the versions
   and edits and deletes them. The last version cannot be deleted.
6. The domain type `ContractSchedule` gives the settings of an interval.
   It reads no clock. The price math functions keep their signatures.
7. The revert path to the TypeScript service ends. New migrations need to
   be compatible only with the running Rust release.

## Alternatives

- **The save time as the start of a version.** Rejected: a user cannot
  enter a change before its date, and a late save prices the days before it
  with the wrong terms.
- **Versions of the time zone and the area too.** Rejected: the local date
  of an interval needs one time zone, and the price read needs one area. No
  current need exists (`VISION.md` → Open Questions: one area per user).
- **An instant (`TIMESTAMPTZ`) as the start of a version.** Rejected: a
  contract changes at a local calendar date. An instant fixes the UTC offset
  of the day of the save and moves when the user changes the time zone.

## Consequences

- A change of the time zone moves the instant at which each version starts.
  A version starts at the local midnight of the current time zone.
- The settings bodies have the new last field `validFrom`. The golden
  replay and `contract.rs` list this difference and the new routes.
- `user_settings` keeps the old term columns, because the release before
  migration 4 reads them during a deploy. The service does not read or
  write them. Their removal is a follow-up.
- During a deploy, the previous release serves for a short time after
  migration 4. A sign-up through it creates a settings row without a
  version; that user gets 500 until a version row exists. A settings save
  through it does not reach `contract_terms`. The instance has one
  household, so the owner avoids both during a deploy.
- A revert PR of this change reads the old term columns, which hold the
  terms at the time of migration 4 (`STACK.md` §15).
- The Better Auth tables `"session"` and `"verification"` and the variable
  `BETTER_AUTH_SECRET` have no use after the end of the revert path. Their
  removal is a follow-up.

## Cost of reversal

Medium. A revert PR restores the one-row behaviour without a data step, but
it loses the terms versions that users saved after migration 4. The table
`contract_terms` stays and is harmless.

## Reassessment

When a user needs more than one delivery area or time zone over time, or
when a contract term needs a start within a day.
