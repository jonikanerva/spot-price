# 0004. One offline evaluation contract for FI forecast changes

- Status: accepted
- Date: 2026-10-20

## Context

Several planned forecast changes need a fair comparison with the current
model. The older `backtest` report scores one issuance per day at 13:00 UTC.
It labels horizons from the series start, compares the model with
persistence on different target sets, and gives no uncertainty. It does not
measure whether a forecast chooses a cheap continuous run.

The data has limits. Stored prices carry no publication time. Fingrid
vintages carry an hour-truncated fetch time. Stored actuals are the latest
revision. The vintage archive covers only a few months.

## Decision

1. `spot-price-tools backtest --eval` gives the evaluation report. The
   report carries the label `forecast-eval v1`. The older report stays
   unchanged, and its numbers are not `forecast-eval v1` numbers.
2. Issuances run at 06:00 and 13:00 UTC by default: before and after the
   day-ahead publication. A horizon `D+k` is the CET/CEST delivery day of
   the target minus the delivery day of the issue time. The report gives
   lead hours, local issue hours in Europe/Helsinki, and input coverage.
3. Accuracy uses paired targets only: a target counts when it has a
   realised price, a forecast of every arm, and both persistence values.
   rMAE is a ratio of sums. It is undefined when the baseline error is 0.
4. A schedule task is one continuous run of 2 h or 4 h at constant power
   inside a local window: 00:00–24:00, or 12:00 to 12:00 of the next day.
   Every arm gets the same task and the same published prices. An arm uses
   its own estimate only after the last published quarter. A task that any
   arm cannot score is excluded for every arm, and the report counts it by
   reason. Regret is the realised cost minus the best feasible hindsight
   cost. The cost basis is the spot price or a declared contract scenario,
   never a stored user setting.
5. Uncertainty comes from a seeded circular block bootstrap over UTC
   issuance days. A day keeps all its issuances. The default block is 7
   days, longer than the target span of one issuance. The interval is the
   95 % percentile interval.
6. `--split <selection-start>,<test-start>` divides issuances by issue
   time. The test segment stays sealed until `--criteria` declares the
   success and regression criteria. The report prints the digest of the
   criteria file. A criteria file looks like this:

   ```json
   {
     "experiment": "lead-matched wind training",
     "declared": "2026-11-01",
     "success": [
       {
         "id": "mae_delta/B-A/13/D+2",
         "statistic": "ci_high",
         "op": "<",
         "value": 0
       }
     ],
     "regression": [
       {
         "id": "mae_delta/B-A/06/D+1",
         "statistic": "ci_low",
         "op": ">",
         "value": 0.05
       }
     ]
   }
   ```

   An `id` names a row of the statistics table. `statistic` is `point`,
   `ci_low`, or `ci_high`. `op` is `<`, `<=`, `>`, or `>=`. An undefined or
   missing statistic is never met. The tool reports outcomes. The owner
   records adopt, reject, or defer.

## Alternatives

- **Change the older report.** Rejected: its numbers are recorded in issues,
  and golden tests hold its output.
- **Diebold–Mariano test with a HAC variance.** Deferred: the issue makes it
  optional. The block bootstrap gives intervals for MAE, rMAE, and costs
  with one method.
- **A random-number crate.** Rejected: SplitMix64 is a few lines, and a fixed
  generator keeps a seed reproducible across versions.

## Consequences

- An experiment can run on a `--db --export` snapshot. The report prints
  the FNV-1a digest of the snapshot text, so the input of a result is
  identifiable.
- The reconstruction assumptions of the data stay visible in every report.
- A comparison of a variant uses `--eval --ab`, so both arms get the same
  issuances, targets, and tasks.

## Cost of reversal

Low. The report is an offline tool. The served forecast does not depend on
it.

## Reassessment

When stored prices get a publication time, or when a forecast change needs
a statistic that the report does not give.
