//! The price-profile families of the within-day shape comparison: their
//! names, the pure-profile arm of the evaluation, and the selection of a
//! weekday-mean window on the selection segment. Pure.

use jiff::tz::TimeZone;
use spot_price_domain::forecast::{
    ExcludedExamples, FLOOR_HISTORY_DAYS, ProfileValues, QuarterStart, SERVED_SHAPE_WEEKS,
    ShapeProfile, profile_values,
};

use crate::backtest::{BacktestError, IssueForecast, IssueTimeInputs, served_window_ms};
use crate::evaluation::{Evaluation, Segment, Split};

const QUARTER_MS: i64 = 15 * 60 * 1000;

/// The fewest weeks of a weekday mean; one week is [`ShapeFamily::PreviousWeek`].
pub const MIN_WEEKDAY_MEAN_WEEKS: u8 = 2;

/// The most weeks of a weekday mean. The served window holds
/// [`FLOOR_HISTORY_DAYS`] of prices before the series start, so a fifth week
/// back is outside it for the first targets.
#[expect(
    clippy::cast_possible_truncation,
    reason = "FLOOR_HISTORY_DAYS / 7 is a small positive number"
)]
pub const MAX_WEEKDAY_MEAN_WEEKS: u8 = (FLOOR_HISTORY_DAYS / 7) as u8;

/// The source of the within-day shape of a forecast arm. The served forecast
/// uses [`ShapeFamily::SERVED`]. The other families than
/// [`ShapeFamily::Persistence`] read local calendar days of the delivery-day
/// zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ShapeFamily {
    /// 24 h earlier, else 168 h earlier (fixed offsets).
    Persistence,
    /// The latest earlier local day with a price for every quarter.
    PreviousDay,
    /// The same weekday one week earlier.
    PreviousWeek,
    /// The mean of the same weekday over this many weeks, from
    /// [`MIN_WEEKDAY_MEAN_WEEKS`] to [`MAX_WEEKDAY_MEAN_WEEKS`].
    WeekdayMean(u8),
}

impl ShapeFamily {
    /// The family of the served forecast.
    pub const SERVED: Self = Self::WeekdayMean(SERVED_SHAPE_WEEKS);

    /// The name in flags, arm names, and the report.
    #[must_use]
    pub fn token(self) -> String {
        match self {
            Self::Persistence => "persistence".to_owned(),
            Self::PreviousDay => "prev_day".to_owned(),
            Self::PreviousWeek => "prev_week".to_owned(),
            Self::WeekdayMean(weeks) => format!("mean{weeks}w"),
        }
    }

    /// Reads a [`ShapeFamily::token`].
    ///
    /// # Errors
    /// A message for another value or a weekday mean outside its range.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let family = match raw {
            "persistence" => Self::Persistence,
            "served" => {
                return Err(format!(
                    "a shape profile cannot be \"served\": the 24 h/168 h rule is persistence, and the served profile is {}",
                    Self::SERVED.token()
                ));
            }
            "prev_day" => Self::PreviousDay,
            "prev_week" => Self::PreviousWeek,
            _ => raw
                .strip_prefix("mean")
                .and_then(|rest| rest.strip_suffix('w'))
                .and_then(|weeks| weeks.parse::<u8>().ok())
                .filter(|weeks| {
                    (MIN_WEEKDAY_MEAN_WEEKS..=MAX_WEEKDAY_MEAN_WEEKS).contains(weeks)
                })
                .map(Self::WeekdayMean)
                .ok_or_else(|| {
                    format!(
                        "a shape profile must be persistence, prev_day, prev_week, or mean<k>w with k from {MIN_WEEKDAY_MEAN_WEEKS} to {MAX_WEEKDAY_MEAN_WEEKS}, got \"{raw}\""
                    )
                })?,
        };
        Ok(family)
    }

    /// The domain profile, on local days of `zone`.
    #[must_use]
    pub const fn profile(self, zone: &TimeZone) -> ShapeProfile<'_> {
        match self {
            Self::Persistence => ShapeProfile::Persistence,
            Self::PreviousDay => ShapeProfile::PreviousPublishedDay(zone),
            Self::PreviousWeek => ShapeProfile::WeekdayMean { zone, weeks: 1 },
            Self::WeekdayMean(weeks) => ShapeProfile::WeekdayMean { zone, weeks },
        }
    }
}

/// The profile prices of every quarter of the served series window of
/// `inputs`, read only from the issue-time FI prices.
///
/// # Errors
/// [`BacktestError`] when the delivery-day time zone is missing.
pub fn profile_at_issue_time(
    inputs: &IssueTimeInputs,
    family: ShapeFamily,
) -> Result<ProfileValues, BacktestError> {
    let zone = crate::backtest::delivery_zone()?;
    let (_, series_end_ms) = served_window_ms(inputs.origin_ms);
    let keys = (inputs.origin_ms..series_end_ms)
        .step_by(usize::try_from(QUARTER_MS).unwrap_or(1))
        .map(QuarterStart::from_millisecond);
    Ok(profile_values(
        family.profile(&zone),
        keys,
        &inputs.fi_prices,
    ))
}

/// The pure-profile forecast of one issuance: the profile price itself, with
/// no model level and no sanity clamp. A quarter without a profile price has
/// no value, so the evaluation pairs no arm on it.
///
/// # Errors
/// [`BacktestError`] when the delivery-day time zone is missing.
pub fn pure_profile_forecast(
    inputs: &IssueTimeInputs,
    family: ShapeFamily,
) -> Result<IssueForecast, BacktestError> {
    let profile = profile_at_issue_time(inputs, family)?;
    let (_, series_end_ms) = served_window_ms(inputs.origin_ms);
    Ok(IssueForecast {
        predicted: profile.values.into_iter().collect(),
        series_start_ms: inputs.origin_ms,
        series_end_ms,
        used_fallback: false,
        sanity_clamped_quarters: 0,
        shape_fallback_quarters: profile.missing,
        training_excluded: ExcludedExamples::default(),
        correction: None,
    })
}

/// The arm name of the served model with the shape of `family`.
#[must_use]
pub fn shape_arm_name(family: ShapeFamily) -> String {
    format!("shape_{}", family.token())
}

/// The arm name of the pure profile of `family`.
#[must_use]
pub fn pure_arm_name(family: ShapeFamily) -> String {
    format!("pure_{}", family.token())
}

/// The pooled MAE of one arm over every paired target of a segment.
#[derive(Debug, Clone, PartialEq)]
pub struct PooledMae {
    pub arm: String,
    /// `None` without a paired target.
    pub mae: Option<f64>,
    pub targets: usize,
}

/// The pooled MAE of every arm over the paired targets that `segment`
/// scores: both issuance hours and every horizon on one target set.
#[must_use]
pub fn pooled_mae(
    evaluation: &Evaluation,
    segment: Segment,
    split: Option<Split>,
) -> Vec<PooledMae> {
    let mut sums = vec![0.0; evaluation.arm_names.len()];
    let mut targets = 0_usize;
    for target in &evaluation.targets {
        let Some(actual) = target.actual else {
            continue;
        };
        if !segment.holds_target(target, split) || !target.complete() {
            continue;
        }
        targets += 1;
        for (sum, prediction) in sums.iter_mut().zip(&target.predictions) {
            *sum += (prediction.unwrap_or(f64::NAN) - actual).abs();
        }
    }
    evaluation
        .arm_names
        .iter()
        .zip(sums)
        .map(|(arm, sum)| PooledMae {
            arm: arm.clone(),
            #[expect(
                clippy::cast_precision_loss,
                reason = "a target count is far below 2^53"
            )]
            mae: (targets > 0).then(|| sum / targets as f64),
            targets,
        })
        .collect()
}

/// The choices of the selection segment; see [`select`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    /// The weekday-mean shape arm with the lowest pooled MAE.
    pub weekday_mean: Option<String>,
    /// The weekday-mean pure arm with the lowest pooled MAE.
    pub pure_weekday_mean: Option<String>,
    /// The arm with the lowest pooled MAE among the reference arm, the
    /// previous-day and previous-week shape arms, and the selected
    /// weekday-mean shape arm.
    pub candidate: Option<String>,
}

fn lowest<'a>(rows: &'a [PooledMae], names: &[String]) -> Option<&'a PooledMae> {
    // The first name wins a tie: a shorter window, or no change.
    names
        .iter()
        .filter_map(|name| rows.iter().find(|r| &r.arm == name && r.mae.is_some()))
        .fold(None, |best: Option<&PooledMae>, row| match best {
            Some(b) if b.mae <= row.mae => Some(b),
            _ => Some(row),
        })
}

/// The declared selection rule of the profile comparison. A weekday-mean
/// window is the one with the lowest pooled MAE; a tie takes fewer weeks.
/// The candidate is the arm with the lowest pooled MAE among `reference`,
/// `shape_prev_day`, `shape_prev_week`, and the selected window; a tie keeps
/// the earlier one, so `reference` wins a tie.
#[must_use]
pub fn select(rows: &[PooledMae], reference: &str) -> Selection {
    let windows = |name: fn(ShapeFamily) -> String| -> Vec<String> {
        (MIN_WEEKDAY_MEAN_WEEKS..=MAX_WEEKDAY_MEAN_WEEKS)
            .map(|k| name(ShapeFamily::WeekdayMean(k)))
            .collect()
    };
    let weekday_mean = lowest(rows, &windows(shape_arm_name)).map(|r| r.arm.clone());
    let pure_weekday_mean = lowest(rows, &windows(pure_arm_name)).map(|r| r.arm.clone());
    let mut names = vec![
        reference.to_owned(),
        shape_arm_name(ShapeFamily::PreviousDay),
        shape_arm_name(ShapeFamily::PreviousWeek),
    ];
    names.extend(weekday_mean.clone());
    let candidate = lowest(rows, &names).map(|r| r.arm.clone());
    Selection {
        weekday_mean,
        pure_weekday_mean,
        candidate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_read_back() {
        for family in [
            ShapeFamily::Persistence,
            ShapeFamily::PreviousDay,
            ShapeFamily::PreviousWeek,
            ShapeFamily::WeekdayMean(2),
            ShapeFamily::WeekdayMean(4),
        ] {
            assert_eq!(ShapeFamily::parse(&family.token()), Ok(family));
        }
        for bad in [
            "",
            "served",
            "mean1w",
            "mean5w",
            "mean2",
            "Prev_day",
            "mean02w x",
        ] {
            assert!(ShapeFamily::parse(bad).is_err(), "{bad}");
        }
        assert_eq!(MAX_WEEKDAY_MEAN_WEEKS, 4);
    }

    fn row(arm: &str, mae: Option<f64>) -> PooledMae {
        PooledMae {
            arm: arm.to_owned(),
            mae,
            targets: 10,
        }
    }

    #[test]
    fn the_selection_takes_the_lowest_pooled_mae_and_breaks_ties_early() {
        let rows = vec![
            row("A", Some(3.0)),
            row("shape_prev_day", Some(3.1)),
            row("shape_prev_week", Some(2.9)),
            row("shape_mean2w", Some(2.8)),
            row("shape_mean3w", Some(2.8)),
            row("shape_mean4w", None),
            row("pure_mean2w", Some(4.0)),
            row("pure_mean3w", Some(3.5)),
        ];
        let got = select(&rows, "A");
        assert_eq!(got.weekday_mean.as_deref(), Some("shape_mean2w"));
        assert_eq!(got.pure_weekday_mean.as_deref(), Some("pure_mean3w"));
        assert_eq!(got.candidate.as_deref(), Some("shape_mean2w"));
        let tie = vec![row("A", Some(3.0)), row("shape_prev_day", Some(3.0))];
        assert_eq!(select(&tie, "A").candidate.as_deref(), Some("A"));
        assert_eq!(
            select(&[], "A"),
            Selection {
                weekday_mean: None,
                pure_weekday_mean: None,
                candidate: None,
            }
        );
    }
}
