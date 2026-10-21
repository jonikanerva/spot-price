//! Feature engineering for the FI price forecast. Every calendar feature is
//! in UTC. The previous-day price statistics use the UTC day unless the
//! feature rules select a local delivery day. Training rows and prediction
//! vectors come from one assembly function, so their columns are identical.

use std::collections::BTreeMap;
use std::f64::consts::PI;

use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;

use crate::areas::Area;
use crate::time::{local_date, start_of_local_day_utc};

use super::js::count_f64;
use super::quarter::{DAY_MS, QuarterMap, QuarterStart, WEEK_MS};

/// How far back from the origin the training rows reach, in days.
pub const TRAINING_HISTORY_DAYS: i64 = 30;

/// Neighbour areas whose lagged prices are features, in column order.
pub const NEIGHBOR_AREAS: [Area; 3] = [Area::Se1, Area::Se3, Area::Ee];

/// The number of feature columns.
pub const FEATURE_COUNT: usize = 21;

/// The feature columns in their fixed order.
pub const FEATURE_NAMES: [&str; FEATURE_COUNT] = [
    "residual_mw",
    "wind_mw",
    "consumption_mw",
    "wind_x_consumption",
    "fi_latest_published",
    "fi_same_quarter_1d",
    "fi_same_quarter_7d",
    "fi_prev_day_min",
    "fi_prev_day_max",
    "fi_prev_day_mean",
    "se1_1d",
    "se1_7d",
    "se3_1d",
    "se3_7d",
    "ee_1d",
    "ee_7d",
    "hour_sin",
    "hour_cos",
    "dow_sin",
    "dow_cos",
    "is_weekend",
];

/// Neighbour spot prices (c/kWh) by area.
pub type NeighborPrices = BTreeMap<Area, QuarterMap>;

/// The inputs of feature assembly. The wind and consumption series already
/// include the tail extension past the published horizon.
#[derive(Debug, Clone, Copy)]
pub struct FeatureContext<'a> {
    /// Published FI spot prices (c/kWh); also the training targets.
    pub fi_prices: &'a QuarterMap,
    /// A missing area or quarter takes the neutral fill.
    pub neighbor_prices: &'a NeighborPrices,
    /// Wind (MW).
    pub wind: &'a QuarterMap,
    /// Consumption (MW).
    pub consumption: &'a QuarterMap,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct DayStats {
    min: f64,
    max: f64,
    mean: f64,
}

/// The published FI price that `fi_latest_published` reads for a quarter.
/// The served forecast uses [`LatestPublishedRule::Before`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LatestPublishedRule {
    /// The last price at or before the quarter. In a training row, this is
    /// the target price of the row itself. The TypeScript parity fixtures
    /// follow this rule.
    AtOrBefore,
    /// The last price strictly before the quarter.
    #[default]
    Before,
}

/// The day whose price statistics `fi_prev_day_min`, `fi_prev_day_max`, and
/// `fi_prev_day_mean` read: the day before the day of the quarter.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PrevDayBasis<'a> {
    /// The UTC calendar day.
    #[default]
    UtcDay,
    /// The local calendar day in this zone. It lasts 23, 24, or 25 hours.
    DeliveryDay(&'a TimeZone),
}

/// [`PrevDayBasis`] with an owned zone, so the price index has no lifetime.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum DayBasis {
    #[default]
    Utc,
    Local(TimeZone),
}

impl From<PrevDayBasis<'_>> for DayBasis {
    fn from(basis: PrevDayBasis<'_>) -> Self {
        match basis {
            PrevDayBasis::UtcDay => Self::Utc,
            PrevDayBasis::DeliveryDay(zone) => Self::Local(zone.clone()),
        }
    }
}

impl DayBasis {
    /// The UTC start (epoch ms) of the day that holds `at`. `None` only
    /// outside the Jiff range.
    fn day_start_ms(&self, at: QuarterStart) -> Option<i64> {
        match self {
            Self::Utc => Some(at.utc_day_start_ms()),
            Self::Local(zone) => local_day_start_ms(local_date(at.timestamp().ok()?, zone), zone),
        }
    }

    /// The UTC start (epoch ms) of the day before the day that holds `at`.
    fn previous_day_start_ms(&self, at: QuarterStart) -> Option<i64> {
        match self {
            Self::Utc => Some(at.utc_day_start_ms() - DAY_MS),
            Self::Local(zone) => {
                let day = local_date(at.timestamp().ok()?, zone);
                local_day_start_ms(day.yesterday().ok()?, zone)
            }
        }
    }
}

fn local_day_start_ms(day: Date, zone: &TimeZone) -> Option<i64> {
    start_of_local_day_utc(day, zone)
        .ok()
        .map(Timestamp::as_millisecond)
}

/// The feature rules of a forecast run. The default is the served forecast.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FeatureRules {
    pub(crate) latest_published: LatestPublishedRule,
    pub(crate) previous_day: DayBasis,
}

/// Lookups derived once from the FI prices of a forecast run, and the feature
/// rules that every row of the run uses.
#[derive(Debug, Clone, PartialEq)]
pub struct PriceIndex {
    stats_by_day_start: BTreeMap<i64, DayStats>,
    neutral: f64,
    rules: FeatureRules,
}

impl PriceIndex {
    /// The mean FI price, used as the neutral fill; 0 with no prices.
    #[must_use]
    pub const fn neutral(&self) -> f64 {
        self.neutral
    }

    /// Whether the previous-day statistics of `at` come from prices, not
    /// from the neutral fill.
    pub(crate) fn has_previous_day(&self, at: QuarterStart) -> bool {
        self.rules
            .previous_day
            .previous_day_start_ms(at)
            .is_some_and(|day_start| self.stats_by_day_start.contains_key(&day_start))
    }
}

/// The shared price index of a forecast run with the default feature rules.
/// The sums run in chronological order of the price keys.
#[must_use]
pub fn build_forecast_price_index(ctx: &FeatureContext<'_>) -> PriceIndex {
    build_price_index(ctx, FeatureRules::default())
}

/// The shared price index with a selected `fi_latest_published` rule and the
/// default previous-day basis. Use it to select the training rule; the
/// TypeScript parity checks use [`LatestPublishedRule::AtOrBefore`].
#[must_use]
pub fn build_price_index_with(
    ctx: &FeatureContext<'_>,
    latest_published: LatestPublishedRule,
) -> PriceIndex {
    build_price_index(
        ctx,
        FeatureRules {
            latest_published,
            ..FeatureRules::default()
        },
    )
}

pub(crate) fn build_price_index(ctx: &FeatureContext<'_>, rules: FeatureRules) -> PriceIndex {
    struct Aggregate {
        min: f64,
        max: f64,
        sum: f64,
        n: usize,
    }
    let mut per_day: BTreeMap<i64, Aggregate> = BTreeMap::new();
    let mut total = 0.0;
    for (&key, &price) in ctx.fi_prices {
        total += price;
        let Some(day_start) = rules.previous_day.day_start_ms(key) else {
            continue;
        };
        per_day
            .entry(day_start)
            .and_modify(|agg| {
                if price < agg.min {
                    agg.min = price;
                }
                if price > agg.max {
                    agg.max = price;
                }
                agg.sum += price;
                agg.n += 1;
            })
            .or_insert(Aggregate {
                min: price,
                max: price,
                sum: price,
                n: 1,
            });
    }
    let count = ctx.fi_prices.len();
    PriceIndex {
        stats_by_day_start: per_day
            .into_iter()
            .map(|(day, agg)| {
                let stats = DayStats {
                    min: agg.min,
                    max: agg.max,
                    mean: agg.sum / count_f64(agg.n),
                };
                (day, stats)
            })
            .collect(),
        neutral: if count > 0 {
            total / count_f64(count)
        } else {
            0.0
        },
        rules,
    }
}

fn cyclical(value: u8, period: f64) -> (f64, f64) {
    let angle = (2.0 * PI * f64::from(value)) / period;
    (angle.sin(), angle.cos())
}

fn assemble_features(at: QuarterStart, ctx: &FeatureContext<'_>, index: &PriceIndex) -> Vec<f64> {
    let neutral = index.neutral;
    // A missing grid value reads 0 MW. The pipeline fills gaps first and
    // counts what remains; see `GridGapRule`.
    let wind = ctx.wind.get(&at).copied().unwrap_or(0.0);
    let consumption = ctx.consumption.get(&at).copied().unwrap_or(0.0);

    let day_ago = at.offset(-DAY_MS);
    let week_ago = at.offset(-WEEK_MS);
    let latest = match index.rules.latest_published {
        LatestPublishedRule::AtOrBefore => ctx.fi_prices.range(..=at).next_back(),
        LatestPublishedRule::Before => ctx.fi_prices.range(..at).next_back(),
    };
    let fi_latest = latest.map_or(neutral, |(_, &price)| price);
    let fi_1d = ctx.fi_prices.get(&day_ago).copied().unwrap_or(neutral);
    let fi_7d = ctx.fi_prices.get(&week_ago).copied().unwrap_or(neutral);
    let previous_day = index
        .rules
        .previous_day
        .previous_day_start_ms(at)
        .and_then(|day_start| index.stats_by_day_start.get(&day_start))
        .copied()
        .unwrap_or(DayStats {
            min: neutral,
            max: neutral,
            mean: neutral,
        });

    let (hour_sin, hour_cos) = cyclical(at.utc_hour(), 24.0);
    let weekday = at.utc_weekday();
    let (dow_sin, dow_cos) = cyclical(weekday, 7.0);
    let is_weekend = if weekday == 0 || weekday == 6 {
        1.0
    } else {
        0.0
    };

    let mut row = Vec::with_capacity(FEATURE_COUNT);
    row.extend([
        consumption - wind,
        wind,
        consumption,
        wind * consumption,
        fi_latest,
        fi_1d,
        fi_7d,
        previous_day.min,
        previous_day.max,
        previous_day.mean,
    ]);
    for area in NEIGHBOR_AREAS {
        let series = ctx.neighbor_prices.get(&area);
        let lag = |key: QuarterStart| series.and_then(|s| s.get(&key)).copied().unwrap_or(neutral);
        row.extend([lag(day_ago), lag(week_ago)]);
    }
    row.extend([hour_sin, hour_cos, dow_sin, dow_cos, is_weekend]);
    row
}

/// The prediction vector of one quarter. It builds the price index on each
/// call; a batch should use [`build_feature_vector_indexed`].
#[must_use]
pub fn build_feature_vector(at: QuarterStart, ctx: &FeatureContext<'_>) -> Vec<f64> {
    let index = build_forecast_price_index(ctx);
    assemble_features(at, ctx, &index)
}

/// The prediction vector of one quarter with a shared price index.
#[must_use]
pub fn build_feature_vector_indexed(
    at: QuarterStart,
    ctx: &FeatureContext<'_>,
    index: &PriceIndex,
) -> Vec<f64> {
    assemble_features(at, ctx, index)
}

/// The training design matrix and its aligned targets.
#[derive(Debug, Clone, PartialEq)]
pub struct BuiltTraining {
    pub feature_names: &'static [&'static str],
    pub rows: Vec<Vec<f64>>,
    pub targets: Vec<f64>,
    /// The quarter of each row, in row order.
    pub keys: Vec<QuarterStart>,
}

/// One row per FI price quarter in `[origin − TRAINING_HISTORY_DAYS, origin)`,
/// in chronological order. `index` defaults to a fresh price index.
#[must_use]
pub fn build_training_matrix(
    origin: Timestamp,
    ctx: &FeatureContext<'_>,
    index: Option<&PriceIndex>,
) -> BuiltTraining {
    training_matrix_at(origin.as_millisecond(), ctx, index)
}

pub(crate) fn training_matrix_at(
    origin_ms: i64,
    ctx: &FeatureContext<'_>,
    index: Option<&PriceIndex>,
) -> BuiltTraining {
    let owned;
    let index = if let Some(index) = index {
        index
    } else {
        owned = build_forecast_price_index(ctx);
        &owned
    };
    let window_start = origin_ms - TRAINING_HISTORY_DAYS * DAY_MS;
    let mut rows = Vec::new();
    let mut targets = Vec::new();
    let mut keys = Vec::new();
    for (&key, &target) in ctx.fi_prices {
        let ms = key.as_millisecond();
        if ms < window_start || ms >= origin_ms {
            continue;
        }
        rows.push(assemble_features(key, ctx, index));
        targets.push(target);
        keys.push(key);
    }
    BuiltTraining {
        feature_names: &FEATURE_NAMES,
        rows,
        targets,
        keys,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forecast::quarter::QUARTER_MS;

    const LATEST: usize = 4;

    fn q(text: &str) -> QuarterStart {
        QuarterStart::floor(text.parse().unwrap())
    }

    struct Owned {
        fi: QuarterMap,
        neighbours: NeighborPrices,
        grid: QuarterMap,
    }

    impl Owned {
        fn new(fi: QuarterMap) -> Self {
            Self {
                fi,
                neighbours: NeighborPrices::new(),
                grid: QuarterMap::new(),
            }
        }

        fn ctx(&self) -> FeatureContext<'_> {
            FeatureContext {
                fi_prices: &self.fi,
                neighbor_prices: &self.neighbours,
                wind: &self.grid,
                consumption: &self.grid,
            }
        }
    }

    /// Eight published quarters with distinct prices 1.0, 2.0, …, 8.0.
    fn eight_quarters() -> (Owned, QuarterStart) {
        let first = q("2026-03-04T10:00:00Z");
        let fi = (0..8_u8)
            .map(|i| (first.offset(i64::from(i) * QUARTER_MS), f64::from(i + 1)))
            .collect();
        (Owned::new(fi), first.offset(8 * QUARTER_MS))
    }

    fn latest_rule(rule: LatestPublishedRule) -> FeatureRules {
        FeatureRules {
            latest_published: rule,
            ..FeatureRules::default()
        }
    }

    #[test]
    fn the_default_latest_published_rule_is_before() {
        assert_eq!(LatestPublishedRule::default(), LatestPublishedRule::Before);
        assert_eq!(
            FeatureRules::default().latest_published,
            LatestPublishedRule::Before
        );
        assert_eq!(FEATURE_NAMES[LATEST], "fi_latest_published");
    }

    #[test]
    fn the_selected_rule_reaches_the_price_index() {
        let (owned, _) = eight_quarters();
        let ctx = owned.ctx();
        for rule in [LatestPublishedRule::AtOrBefore, LatestPublishedRule::Before] {
            assert_eq!(
                build_price_index_with(&ctx, rule),
                build_price_index(&ctx, latest_rule(rule))
            );
        }
        assert_eq!(
            build_forecast_price_index(&ctx),
            build_price_index_with(&ctx, LatestPublishedRule::Before)
        );
    }

    #[test]
    fn at_or_before_gives_each_training_row_its_own_target() {
        let (owned, series_start) = eight_quarters();
        let ctx = owned.ctx();
        let index = build_price_index(&ctx, latest_rule(LatestPublishedRule::AtOrBefore));
        let training = training_matrix_at(series_start.as_millisecond(), &ctx, Some(&index));
        assert_eq!(training.rows.len(), 8);
        for (row, target) in training.rows.iter().zip(&training.targets) {
            assert_eq!(row[LATEST].to_bits(), target.to_bits());
        }
    }

    #[test]
    fn before_gives_each_training_row_the_previous_published_quarter() {
        let (owned, series_start) = eight_quarters();
        let ctx = owned.ctx();
        let index = build_price_index(&ctx, latest_rule(LatestPublishedRule::Before));
        let training = training_matrix_at(series_start.as_millisecond(), &ctx, Some(&index));
        let got: Vec<f64> = training.rows.iter().map(|row| row[LATEST]).collect();
        // The first row has no earlier price, so it takes the neutral fill.
        assert_eq!(index.neutral(), 4.5);
        assert_eq!(got, [4.5, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
        for (row, target) in training.rows.iter().zip(&training.targets) {
            assert_ne!(row[LATEST].to_bits(), target.to_bits());
        }
    }

    #[test]
    fn before_skips_a_gap_to_the_last_published_quarter() {
        let (mut owned, series_start) = eight_quarters();
        let gap = series_start.offset(-3 * QUARTER_MS);
        owned.fi.remove(&gap);
        let ctx = owned.ctx();
        let index = build_price_index(&ctx, latest_rule(LatestPublishedRule::Before));
        let after_gap = gap.offset(QUARTER_MS);
        assert_eq!(
            build_feature_vector_indexed(after_gap, &ctx, &index)[LATEST],
            5.0
        );
    }

    #[test]
    fn both_latest_published_rules_give_the_same_predicted_rows() {
        let (owned, series_start) = eight_quarters();
        let ctx = owned.ctx();
        let at_or_before = build_price_index(&ctx, latest_rule(LatestPublishedRule::AtOrBefore));
        let before = build_price_index(&ctx, latest_rule(LatestPublishedRule::Before));
        for step in [0, 1, 95, 96 * 3 - 1] {
            let at = series_start.offset(step * QUARTER_MS);
            let a = build_feature_vector_indexed(at, &ctx, &at_or_before);
            let b = build_feature_vector_indexed(at, &ctx, &before);
            assert_eq!(a[LATEST], 8.0, "step {step}");
            let bits = |row: &[f64]| row.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
            assert_eq!(bits(&a), bits(&b), "step {step}");
        }
    }

    const PREV_MIN: usize = 7;
    const PREV_MAX: usize = 8;
    const PREV_MEAN: usize = 9;

    fn oslo() -> TimeZone {
        TimeZone::get("Europe/Oslo").unwrap()
    }

    fn day_rule(basis: PrevDayBasis<'_>) -> FeatureRules {
        FeatureRules {
            previous_day: basis.into(),
            ..FeatureRules::default()
        }
    }

    /// Every quarter in `[from, to)` at `fill`, then the `marks` on top.
    fn prices(from: &str, to: &str, fill: f64, marks: &[(&str, f64)]) -> QuarterMap {
        let (from, to) = (q(from), q(to));
        let mut fi = QuarterMap::new();
        let mut at = from;
        while at < to {
            fi.insert(at, fill);
            at = at.offset(QUARTER_MS);
        }
        for &(key, value) in marks {
            assert!(fi.insert(q(key), value).is_some(), "{key}");
        }
        fi
    }

    fn quarters_in(fi: &QuarterMap, from: &str, to: &str) -> usize {
        fi.range(q(from)..q(to)).count()
    }

    /// `(min, max, mean)` of the previous day for the quarter `at`.
    fn previous_day_stats(owned: &Owned, rules: FeatureRules, at: &str) -> (f64, f64, f64) {
        let ctx = owned.ctx();
        let index = build_price_index(&ctx, rules);
        let row = build_feature_vector_indexed(q(at), &ctx, &index);
        (row[PREV_MIN], row[PREV_MAX], row[PREV_MEAN])
    }

    #[test]
    fn the_default_prev_day_basis_is_the_utc_day() {
        assert_eq!(PrevDayBasis::default(), PrevDayBasis::UtcDay);
        assert_eq!(FeatureRules::default().previous_day, DayBasis::Utc);
        assert_eq!(
            FEATURE_NAMES[PREV_MIN..=PREV_MEAN],
            ["fi_prev_day_min", "fi_prev_day_max", "fi_prev_day_mean"]
        );
    }

    #[test]
    fn delivery_day_statistics_cover_the_23_hour_day_of_march_2026() {
        // Delivery day 2026-03-29 runs from 2026-03-28T23:00Z to 2026-03-29T22:00Z.
        let owned = Owned::new(prices(
            "2026-03-27T00:00:00Z",
            "2026-03-31T00:00:00Z",
            2.0,
            &[
                ("2026-03-28T22:45:00Z", 50.0),
                ("2026-03-28T23:00:00Z", 10.0),
                ("2026-03-29T21:45:00Z", -4.0),
                ("2026-03-29T22:00:00Z", -50.0),
            ],
        ));
        let zone = oslo();
        let delivery = || day_rule(PrevDayBasis::DeliveryDay(&zone));
        assert_eq!(
            quarters_in(&owned.fi, "2026-03-28T23:00:00Z", "2026-03-29T22:00:00Z"),
            92
        );
        let march_29 = (-4.0, 10.0, (10.0 - 4.0 + 90.0 * 2.0) / 92.0);
        assert_eq!(
            previous_day_stats(&owned, delivery(), "2026-03-30T10:00:00Z"),
            march_29
        );
        assert_eq!(
            previous_day_stats(&owned, delivery(), "2026-03-29T22:00:00Z"),
            march_29
        );
        // The last quarter of 2026-03-29 reads the 24-hour day 2026-03-28.
        assert_eq!(
            previous_day_stats(&owned, delivery(), "2026-03-29T21:45:00Z"),
            (2.0, 50.0, (95.0 * 2.0 + 50.0) / 96.0)
        );
    }

    #[test]
    fn delivery_day_statistics_cover_the_25_hour_day_of_october_2026() {
        // Delivery day 2026-10-25 runs from 2026-10-24T22:00Z to 2026-10-25T23:00Z.
        let owned = Owned::new(prices(
            "2026-10-23T00:00:00Z",
            "2026-10-27T00:00:00Z",
            2.0,
            &[
                ("2026-10-24T21:45:00Z", 50.0),
                ("2026-10-24T22:00:00Z", 10.0),
                ("2026-10-25T22:45:00Z", -4.0),
                ("2026-10-25T23:00:00Z", -50.0),
            ],
        ));
        let zone = oslo();
        let delivery = || day_rule(PrevDayBasis::DeliveryDay(&zone));
        assert_eq!(
            quarters_in(&owned.fi, "2026-10-24T22:00:00Z", "2026-10-25T23:00:00Z"),
            100
        );
        let october_25 = (-4.0, 10.0, (10.0 - 4.0 + 98.0 * 2.0) / 100.0);
        assert_eq!(
            previous_day_stats(&owned, delivery(), "2026-10-26T10:00:00Z"),
            october_25
        );
        assert_eq!(
            previous_day_stats(&owned, delivery(), "2026-10-25T23:00:00Z"),
            october_25
        );
        // The last quarter of 2026-10-25 reads the 24-hour day 2026-10-24.
        assert_eq!(
            previous_day_stats(&owned, delivery(), "2026-10-25T22:45:00Z"),
            (2.0, 50.0, (95.0 * 2.0 + 50.0) / 96.0)
        );
    }

    #[test]
    fn at_the_cet_series_start_only_the_delivery_day_basis_reads_a_complete_day() {
        // The series starts at 2026-02-10T23:00Z, the start of delivery day
        // 2026-02-11. The last published hour, 2026-02-09T23:00Z to 24:00Z,
        // is in delivery day 2026-02-10 but in UTC day 2026-02-09.
        let owned = Owned::new(prices(
            "2026-02-08T23:00:00Z",
            "2026-02-10T23:00:00Z",
            1.0,
            &[
                ("2026-02-09T23:00:00Z", 9.0),
                ("2026-02-09T23:15:00Z", 9.0),
                ("2026-02-09T23:30:00Z", 9.0),
                ("2026-02-09T23:45:00Z", 9.0),
            ],
        ));
        assert_eq!(
            quarters_in(&owned.fi, "2026-02-10T00:00:00Z", "2026-02-11T00:00:00Z"),
            92
        );
        assert_eq!(
            quarters_in(&owned.fi, "2026-02-09T23:00:00Z", "2026-02-10T23:00:00Z"),
            96
        );
        let first_full_utc_day = "2026-02-11T12:00:00Z";
        assert_eq!(
            previous_day_stats(&owned, FeatureRules::default(), first_full_utc_day),
            (1.0, 1.0, 1.0)
        );
        let zone = oslo();
        assert_eq!(
            previous_day_stats(
                &owned,
                day_rule(PrevDayBasis::DeliveryDay(&zone)),
                first_full_utc_day
            ),
            (1.0, 9.0, (92.0 + 4.0 * 9.0) / 96.0)
        );
    }
}
