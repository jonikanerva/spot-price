//! Feature engineering for the FI price forecast. Every calendar feature is
//! in UTC. Training rows and prediction vectors come from one assembly
//! function, so their columns are identical.

use std::collections::BTreeMap;
use std::f64::consts::PI;

use jiff::Timestamp;

use crate::areas::Area;

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

/// Lookups derived once from the FI prices of a forecast run.
#[derive(Debug, Clone, PartialEq)]
pub struct PriceIndex {
    stats_by_day_start: BTreeMap<i64, DayStats>,
    neutral: f64,
}

impl PriceIndex {
    /// The mean FI price, used as the neutral fill; 0 with no prices.
    #[must_use]
    pub const fn neutral(&self) -> f64 {
        self.neutral
    }
}

/// The shared price index of a forecast run. The sums run in chronological
/// order of the price keys.
#[must_use]
pub fn build_forecast_price_index(ctx: &FeatureContext<'_>) -> PriceIndex {
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
        per_day
            .entry(key.utc_day_start_ms())
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
    }
}

fn cyclical(value: u8, period: f64) -> (f64, f64) {
    let angle = (2.0 * PI * f64::from(value)) / period;
    (angle.sin(), angle.cos())
}

fn assemble_features(at: QuarterStart, ctx: &FeatureContext<'_>, index: &PriceIndex) -> Vec<f64> {
    let neutral = index.neutral;
    let wind = ctx.wind.get(&at).copied().unwrap_or(0.0);
    let consumption = ctx.consumption.get(&at).copied().unwrap_or(0.0);

    let day_ago = at.offset(-DAY_MS);
    let week_ago = at.offset(-WEEK_MS);
    let fi_latest = ctx
        .fi_prices
        .range(..=at)
        .next_back()
        .map_or(neutral, |(_, &price)| price);
    let fi_1d = ctx.fi_prices.get(&day_ago).copied().unwrap_or(neutral);
    let fi_7d = ctx.fi_prices.get(&week_ago).copied().unwrap_or(neutral);
    let previous_day = index
        .stats_by_day_start
        .get(&(at.utc_day_start_ms() - DAY_MS))
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
