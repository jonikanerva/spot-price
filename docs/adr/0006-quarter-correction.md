# 0006. A quarter correction after the shape step of the FI forecast

- Status: accepted for the offline comparison; the served forecast does not use it
- Date: 2026-10-21

## Context

The served forecast keeps the UTC-day mean of the ridge model and replaces
the within-day variation with a historical price profile. So a different
future wind pattern inside one UTC day cannot change the relative prices of
the quarters of that day. The served profile is the 4-week same-weekday
mean (`mean4w`).

A correction must be a closed-form linear estimator (`VISION.md`). It must
learn only from the information that each training issuance had (ADR 0005).

The served price window holds 30 days. A `mean4w` profile price needs the
same local time 1–4 weeks back. A training target `q` of a run with series
start `S` is in `[S − 30 d, S)`. Its profile price needs prices from
`q − 28 d`, so only targets from `S − 2 d` have one in the served window.

## Decision

1. `ForecastOptions.correction` selects the quarter correction. The default
   is `None`, the served forecast.
2. The baseline `b(q)` is the profile price of the shape profile of the run.
   A training example takes `b(q)` from the FI prices published at its
   issuance. An example without one is left out and counted.
3. The examples are the issuance training examples of ADR 0005, plus the
   column `baseline_price`, and optionally a Finnish public-holiday column
   (local date in Europe/Helsinki; the statutory days off plus Midsummer Eve
   and Christmas Eve).
4. The examples read prices and actuals up to `CORRECTION_HISTORY_DAYS`
   (34) days before the served window. The served inputs of the run, the
   sanity clamp, and the neutral fill do not change.
5. Two decompositions:
   - Joint: one ridge fit, no target transform, on `realised − b(q)`.
     The output is `b(q) + c(q)`. The correction sets the daily level and
     the within-day shape.
   - DayDeviation: a ridge fit on the within-day deviation
     `(y − ȳ) − (b − b̄)` per training issuance and UTC day, with demeaned
     features. The output is the shape-step output plus `d(q) − d̄`. The
     daily level of the model stays.
6. The correction runs after the shape step and before the sanity clamp.
   A fit with fewer than `MIN_FIT_SAMPLES` examples, or no solution, leaves
   the shape-step output. A run quarter without `b(q)` keeps it too.
7. The ridge penalty, the training window (at most 30 days), and a recency
   half-life are settings. `fit_ridge_weighted` solves
   `(XᵀWX + λI′) β = XᵀWy` with weighted standardization.
8. The evaluation compares the served forecast (arm A, the baseline with
   the daily level of the model) and the correction arms on the same
   targets. The selection segment selects one setting from a predeclared
   grid. The report repeats its statistics under `corr_selected`, so the
   criteria file names it before the run.

## Alternatives

- **The served model output as the baseline of the joint fit.** Rejected
  for now: each training issuance needs its own served fit, and the old
  issuances need grid actuals from before the read window.
- **Read a wider price window into the served inputs.** Rejected: arm A
  and the correction arms would get different clamps and neutral fills.
- **Separate fits per lead range.** Deferred, as in ADR 0005.
- **Holiday column in every correction arm.** Rejected: the issue asks for
  a separate arm. The evaluation window holds few holidays.

## Consequences

- One recompute with a correction took about 85–105 ms and about 55 MB of
  extra peak memory on archive-sized synthetic input (release build, macOS
  arm64). The served recompute took about 4–5 ms on the same input.
- To serve the correction, the server needs about 34 more days of prices
  and actuals, and the vintage ladder of ADR 0005.

## Cost of reversal

Low. The served default does not change. Removing the option removes two
domain modules, one tools module, and the `--corrections` flags.

## Reassessment

After the paired production run, or when the served price window changes.
