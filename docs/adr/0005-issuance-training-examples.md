# 0005. Issuance training examples for the FI forecast

- Status: accepted for the offline comparison; the served forecast does not use it
- Date: 2026-10-20

## Context

The served forecast fits on one row per published FI quarter of the last
30 days. A row has the inputs that are known now: settled wind and
consumption, and every price lag. A prediction two or three days ahead has
other inputs: a Fingrid forecast with a long lead or the tail extension,
and neutral fills for the lags that are not published yet. So the fit
learns weights for inputs that the prediction does not have.

Later experiments need the same rule for what a forecast knew at an
issuance. They must not each build their own rule.

## Decision

1. `TrainingRule::Issuance` in `ForecastOptions` selects issuance training.
   The default stays `TrainingRule::Realised`, the served forecast.
2. A training example is an earlier issuance `i` and one target quarter of
   its window. The issuances are whole days before the run, so they have
   the time of day of the run.
3. Issuance `i` has the inputs of the served forecast at `i`, from the data
   of the run:
   - FI and neighbour prices published at `i`: delivery day D is published
     at 12:00 UTC on D − 1 (`publication_horizon`). The series start `S_i`
     is one quarter after the last such FI price.
   - Fingrid 245 and 165 vintages issued strictly before `i`, newest per
     target. `issued_at` is the hour-truncated fetch time.
   - Fingrid actuals that started strictly before `i`.
4. An example target is in `[S_i, S_i + 3 days)`, inside the 30-day
   training window, and has a published price. A target never takes an
   actual in place of a forecast.
5. The 21 served feature columns use the served assembly and fills. Ten
   columns follow: the lead in hours, wind × lead, consumption × lead, two
   vintage-source indicators, and five missing-lag indicators. One ridge fit
   covers all leads. The estimator, the within-day shape, and the clamp do
   not change.
6. The prediction rows use the same assembly at the issue time of the run.
7. `build_issuance_examples` returns the examples with their issuance,
   series start, target, lead, input sources, lag availability, features,
   and target price. Later experiments reuse it.

## Alternatives

- **Separate fits per lead range.** Deferred: the issue allows them only
  when the data supports them. One shared fit with a lead column and lead
  interactions comes first.
- **A wider price window for the training issuances.** Rejected for this
  comparison: arm B would read other data than arm A and the served
  forecast. An old issuance sees only the part of its window inside the
  served window; its summary shows that.
- **Hourly training issuances.** Rejected for now: the rows grow 24 times
  for little new information.

## Consequences

- The paired comparison runs with `backtest --eval --ab training=issuance`.
- One recompute with issuance training took about 30 ms and about 30 MB of
  extra peak memory on archive-sized input (release build, macOS arm64).
- To serve issuance training, the server needs a read of every vintage of
  the training window. The current live read returns one vintage per
  target.

## Cost of reversal

Low. The served default does not change. Removing the option removes one
module and one `--ab` key.

## Reassessment

After the paired production run on cold-season data, or when stored prices
get a publication time.
