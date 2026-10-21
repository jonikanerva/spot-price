# 0007. Grid gaps in the FI forecast take an earlier week of actuals

- Status: accepted
- Date: 2026-10-21

## Context

The forecast features read a wind and a consumption value for each quarter.
The value comes from a Fingrid forecast vintage, a stored actual, or the
tail extension after the last forecast quarter. The consumption extension
reads the same quarter 1 week earlier. The wind extension reads the mean of
the same quarter 1–4 weeks earlier. Dataset 165 never reaches an
unpublished target, so every predicted quarter takes consumption from the
extension.

When the source quarter is missing, the feature read 0 MW. No feature or
counter marked it. A missing day of consumption actuals one week before the
predicted window gave 96 predicted quarters with 0 MW consumption (#171).
`DOCTRINE.md` P5 forbids replacing missing information with an asserted
value without a mark.

## Decision

1. `GridGapRule::EarlierWeeks` is the served rule. A wind or consumption
   quarter without a vintage, an actual, or an extension value takes the
   mean of the nearest earlier weeks of actuals that hold a value. It takes
   as many weeks as the extension of the dataset (1 for consumption, 4 for
   wind) and looks at most `GAP_FILL_LOOKBACK_WEEKS` (4) weeks back. When
   the extension weeks all hold a value, the result equals the extension.
2. The rule covers every quarter from the first stored actual to the series
   end: the training rows and the predicted quarters read the same merged
   series. A training row in a hole of the stored data so reads an earlier
   week, not the realised value. This is an assumption of equal treatment,
   not a measured improvement.
3. A quarter that no earlier week fills still reads 0 MW. The diagnostics
   count it (`grid_missing_quarters`, `training_grid_missing_rows`), and a
   recompute with a non-zero count logs `forecast_grid_missing`. A
   gap-filled quarter is an extension quarter for issuance training.
4. `GridGapRule::Zero` keeps the TypeScript rule. The TypeScript parity
   tests select it, as they select the persistence shape.
5. The v1 response does not change: no new reason, no new degraded state.

## Alternatives

- **Leave out a training row or a predicted quarter without a value.** Not
  chosen. With a long outage of one dataset, the fit falls back or the
  series is zero-seeded. Then `degraded` and `reason` change, a v1
  contract change that needs an owner decision.
- **Degrade the response for a quarter without a value.** Not chosen for
  the same reason. It needs a new reason text.
- **A neutral fill such as the mean consumption.** Rejected: it asserts a
  value that no source gave, without a better basis than an earlier week.

## Consequences

- A hole of up to 3 consecutive weeks at the same quarter no longer gives
  0 MW. The Fingrid job fetches the last 30 days of actuals every hour, so
  a remaining 0 MW needs a long Fingrid outage.
- A run without any forecast vintage now reads consumption and wind from
  earlier weeks of actuals instead of 0 MW. Old issuances in issuance
  training then give examples where the input has the actuals.
- The residual 0 MW quarter is visible only in the log and the
  diagnostics, not in the API response.

## Cost of reversal

Low. Set the served default to `GridGapRule::Zero`.

## Reassessment

When `forecast_grid_missing` appears in the production log, or when the
owner decides how the API answers a quarter without grid input.
