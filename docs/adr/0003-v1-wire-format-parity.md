# 0003. The v1 API keeps the TypeScript wire format

- Status: accepted
- Date: 2026-10-14

## Context

Home Assistant templates read the `/api/v1` responses. A template can show
`5` and `5.0` differently, and can compare strings. The TypeScript service
wrote JSON with JavaScript number formatting, instants with milliseconds,
and errors as `{"error": "..."}`. Prices are `DOUBLE PRECISION` in the
database and binary floats in the calculations.

The default Rust profile (STACK-RUST §2.2, §7) prefers RFC 9457 Problem
Details and exact decimals.

## Decision

1. `/api/v1`, `/api/public/spot`, and the session and key routes keep the
   TypeScript wire format byte for byte. Numbers use the JavaScript
   `String(number)` form, and negative zero is written as `0`. Instants use
   `YYYY-MM-DDTHH:MM:SS.sssZ`. Errors stay `{"error": "..."}`.
2. Price math uses `f64` and emulates JavaScript `Math.round` for the
   three-decimal rounding. Golden files from the TypeScript modules prove
   the parity.
3. A breaking change to the wire format needs `/api/v2` (`VISION.md`).

## Alternatives

- **Problem Details in v1.** Rejected: it breaks existing automations.
- **Exact decimals.** Rejected for the port: the stored values are binary
  floats, a column change is a data migration that breaks the revert path,
  and the rounding results would differ from the bills that users compare.

## Consequences

- The server has one JSON writer for all responses, with tests for negative
  zero, whole numbers, and shortest round-trip output.
- The forecast can differ from the TypeScript forecast by a few units in the
  last place before rounding, because transcendental functions differ
  between V8 and the platform maths library. The golden tests use a relative
  tolerance of 1e-9 before rounding and exact equality after it.

## Cost of reversal

Medium. A v2 API can adopt Problem Details and decimals later.

## Reassessment

When a v2 API is planned.
