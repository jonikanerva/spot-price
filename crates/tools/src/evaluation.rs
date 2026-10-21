//! The FI forecast evaluation of the common evaluation contract: several
//! issuances per day, accuracy per delivery-day horizon against persistence
//! on identical targets, and feasible-schedule costs. Pure. It reuses the
//! issue-time reconstruction of [`crate::backtest`], so every arm sees only
//! what the served route knew at the issue time.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use spot_price_domain::forecast::{CorrectionDiagnostics, ExcludedExamples, FORECAST_DAYS};
use spot_price_domain::time::{local_hour, next_calendar_day, start_of_local_day_utc};

use crate::backtest::{
    AsOfMode, BacktestError, Baseline, CorrectionCache, ForecastVariant, IssueTimeInputs,
    OutOfRange, baseline_for, baseline_zone, delivery_zone, find_leaking_inputs,
    forecast_at_issue_time_cached, issue_has_grid_data, publication_horizon_ms,
    reconstruct_issue_time, select_vintages_as_of,
};
use crate::bootstrap::{BootstrapOptions, Estimate, UnitSums, estimate, resample_indices};
use crate::data::{BacktestData, DATASET_CONSUMPTION_FORECAST, DATASET_WIND_FORECAST};
use crate::profile::{profile_at_issue_time, pure_profile_forecast};
use crate::schedule::{
    CostBasis, HOUSEHOLD_TIME_ZONE, HouseholdTimeZone, Pricer, QUARTER_MS, ScheduleError,
    ScheduleTask, cheapest_start, run_mean_price,
};

const HOUR_MS: i64 = 60 * 60 * 1000;
const DAY_MS: i64 = 24 * HOUR_MS;

/// The label of this evaluation contract. A result without it comes from an
/// older report and is not comparable row by row.
pub const EVAL_VERSION: &str = "forecast-eval v1";

/// The default issue hours (UTC): before and after the day-ahead
/// publication.
pub const DEFAULT_ISSUANCE_HOURS_UTC: [u8; 2] = [6, 13];

/// The run lengths of the schedule comparison: 2 h and 4 h.
pub const SCHEDULE_QUARTERS: [usize; 2] = [8, 16];

/// The declared constant power of a scheduled run. Costs are means per kWh,
/// so they do not depend on it; the energy of a run is power × length.
pub const POWER_KW: f64 = 1.0;

/// The local hour of the across-day window start and deadline.
pub const ACROSS_DAY_HOUR: i8 = 12;

/// The issuance error of an evaluation run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EvalError {
    #[error(transparent)]
    Backtest(#[from] BacktestError),
    #[error(transparent)]
    HouseholdTimeZone(#[from] HouseholdTimeZone),
}

impl From<OutOfRange> for EvalError {
    fn from(error: OutOfRange) -> Self {
        Self::Backtest(BacktestError::OutOfRange(error))
    }
}

/// What an arm forecasts with.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ArmKind {
    /// The forecast model of the variant.
    #[default]
    Model,
    /// Only the shape profile of the variant: historical prices with no
    /// model level and no learned correction.
    PureProfile,
}

/// A forecast model under evaluation.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelArm {
    pub name: String,
    pub variant: ForecastVariant,
    pub kind: ArmKind,
}

/// The window of a scheduled run, in [`HOUSEHOLD_TIME_ZONE`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TaskKind {
    /// From 00:00 to 24:00 of one local day.
    WithinDay,
    /// From 12:00 of one local day to 12:00 of the next one.
    AcrossDay,
}

impl TaskKind {
    pub const ALL: [Self; 2] = [Self::WithinDay, Self::AcrossDay];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::WithinDay => "within_day",
            Self::AcrossDay => "across_day",
        }
    }
}

/// The options of [`evaluate`].
#[derive(Debug, Clone, PartialEq)]
pub struct EvalOptions {
    pub issuance_hours_utc: Vec<u8>,
    /// The first arm is the reference of the arm deltas.
    pub arms: Vec<ModelArm>,
    pub cost: CostBasis,
    /// More arm deltas: `(arm, reference)` by arm name. A name that is not
    /// an arm adds nothing.
    pub paired: Vec<(String, String)>,
}

/// The state of one candidate issuance.
#[derive(Debug, Clone, PartialEq)]
pub enum IssueStatus {
    /// No wind forecast vintage was knowable: the archive had not started.
    PreVintage,
    /// The served route would answer `NoGridData`.
    NoGridData,
    Forecast(ForecastIssue),
}

/// One issuance with a forecast of every arm.
#[derive(Debug, Clone, PartialEq)]
pub struct ForecastIssue {
    pub leak_free: bool,
    /// Per arm.
    pub fallback: Vec<bool>,
    /// Per arm.
    pub clamped_quarters: Vec<usize>,
    /// The quarters of the served series window.
    pub expected_quarters: usize,
    /// Per arm: the quarters with a forecast value.
    pub forecast_quarters: Vec<usize>,
    /// Per arm: the quarters with no profile price.
    pub shape_fallback: Vec<usize>,
    /// Per arm: the quarters whose profile source is in the skipped hour of
    /// a 23-hour day. Always 0 for the fixed offsets of the served profile.
    pub dst_gap: Vec<usize>,
    /// Per arm: issuance-training targets left out for a missing grid value.
    pub training_excluded: Vec<ExcludedExamples>,
    /// Per arm: the quarter correction, if the arm has one.
    pub correction: Vec<Option<CorrectionDiagnostics>>,
}

/// One candidate issuance.
#[derive(Debug, Clone, PartialEq)]
pub struct IssueRecord {
    pub t_ms: i64,
    pub hour_utc: u8,
    /// The issue hour in [`HOUSEHOLD_TIME_ZONE`].
    pub local_hour: u8,
    pub status: IssueStatus,
}

/// One forecast target of one issuance.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetRecord {
    pub t_ms: i64,
    pub hour_utc: u8,
    /// The delivery day of the target minus the delivery day of the issue
    /// time, both in the CET/CEST delivery-day zone.
    pub day_ahead: i64,
    pub target_ms: i64,
    pub actual: Option<f64>,
    /// Per arm.
    pub predictions: Vec<Option<f64>>,
    pub last_week: Option<f64>,
    pub last_published_day: Option<f64>,
    pub wind_vintage: bool,
    pub consumption_vintage: bool,
}

impl TargetRecord {
    /// The hours from the issue time to the target start.
    #[must_use]
    pub fn lead_hours(&self) -> f64 {
        #[expect(clippy::cast_precision_loss, reason = "a lead in ms is far below 2^53")]
        let hours = (self.target_ms - self.t_ms) as f64 / HOUR_MS as f64;
        hours
    }

    /// Whether every arm and both baselines can be scored on this target.
    #[must_use]
    pub fn complete(&self) -> bool {
        self.actual.is_some()
            && self.last_week.is_some()
            && self.last_published_day.is_some()
            && self.predictions.iter().all(Option::is_some)
    }
}

/// Why a target or a task is left out of the paired comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Exclusion {
    /// Every quarter of the window was published at the issue time; no
    /// forecast is needed.
    FullyPublished,
    /// The window is shorter than the run.
    Infeasible,
    /// A quarter has no realised price.
    MissingRealised,
    /// The model arm (index) has no forecast for a quarter.
    MissingForecast(usize),
    /// The persistence baseline has no value for a quarter.
    MissingLastWeek,
    MissingLastPublishedDay,
}

/// The schedule arms after the model arms, in order.
pub const BASELINE_SCHEDULE_ARMS: [&str; 4] = [
    "last_week",
    "last_published_day",
    "earliest_start",
    "hindsight",
];

/// One schedule task of one issuance.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskRecord {
    pub t_ms: i64,
    pub hour_utc: u8,
    pub kind: TaskKind,
    pub task: ScheduleTask,
    /// Window quarters that were published at the issue time.
    pub published_quarters: usize,
    pub window_quarters: usize,
    /// The realised mean cost (c/kWh) per schedule arm: the model arms, then
    /// [`BASELINE_SCHEDULE_ARMS`].
    pub outcome: Result<Vec<f64>, Exclusion>,
}

impl TaskRecord {
    /// The hours from the issue time to the earliest start.
    #[must_use]
    pub fn lead_hours(&self) -> f64 {
        #[expect(clippy::cast_precision_loss, reason = "a lead in ms is far below 2^53")]
        let hours = (self.task.earliest_start_ms - self.t_ms) as f64 / HOUR_MS as f64;
        hours
    }
}

/// All records of one evaluation run.
#[derive(Debug, Clone, PartialEq)]
pub struct Evaluation {
    pub arm_names: Vec<String>,
    /// The extra arm deltas `(arm, reference)` by arm index; see
    /// [`EvalOptions::paired`].
    pub paired: Vec<(usize, usize)>,
    pub issuance_hours_utc: Vec<u8>,
    pub issues: Vec<IssueRecord>,
    pub targets: Vec<TargetRecord>,
    pub tasks: Vec<TaskRecord>,
}

impl Evaluation {
    /// The model arms, then [`BASELINE_SCHEDULE_ARMS`].
    #[must_use]
    pub fn schedule_arm_names(&self) -> Vec<String> {
        self.arm_names
            .iter()
            .cloned()
            .chain(BASELINE_SCHEDULE_ARMS.iter().map(|s| (*s).to_owned()))
            .collect()
    }
}

fn instant(ms: i64) -> Result<Timestamp, OutOfRange> {
    Timestamp::from_millisecond(ms).map_err(|_| OutOfRange)
}

fn household_zone() -> Result<TimeZone, HouseholdTimeZone> {
    TimeZone::get(HOUSEHOLD_TIME_ZONE).map_err(|_| HouseholdTimeZone)
}

fn date_in(ms: i64, zone: &TimeZone) -> Result<Date, OutOfRange> {
    Ok(instant(ms)?.to_zoned(zone.clone()).date())
}

fn days_between(from: Date, to: Date) -> Result<i64, OutOfRange> {
    from.until(to)
        .map(|span| i64::from(span.get_days()))
        .map_err(|_| OutOfRange)
}

/// The fixed context of one run.
struct Run<'a> {
    data: &'a BacktestData,
    options: &'a EvalOptions,
    pricer: Pricer,
    realised: BTreeMap<i64, f64>,
    delivery: TimeZone,
    household: TimeZone,
    baseline: TimeZone,
}

/// Runs the evaluation: for each UTC day from the first to the last FI price
/// and each issue hour, one issuance with the issue-time inputs.
///
/// # Errors
/// [`EvalError`] when an instant leaves the Jiff range or a time zone is
/// missing.
pub fn evaluate(data: &BacktestData, options: &EvalOptions) -> Result<Evaluation, EvalError> {
    let realised: BTreeMap<i64, f64> = data
        .prices
        .iter()
        .filter(|p| p.area == "FI")
        .map(|p| (p.start.as_millisecond(), p.spot_cents_kwh))
        .collect();
    let run = Run {
        data,
        options,
        pricer: Pricer::new(&options.cost)?,
        realised,
        delivery: delivery_zone()?,
        household: household_zone()?,
        baseline: baseline_zone()?,
    };
    let arm_names: Vec<String> = options.arms.iter().map(|a| a.name.clone()).collect();
    let index_of = |name: &str| arm_names.iter().position(|n| n == name);
    let paired = options
        .paired
        .iter()
        .filter_map(|(arm, reference)| Some((index_of(arm)?, index_of(reference)?)))
        .collect();
    let mut evaluation = Evaluation {
        paired,
        arm_names,
        issuance_hours_utc: options.issuance_hours_utc.clone(),
        issues: Vec::new(),
        targets: Vec::new(),
        tasks: Vec::new(),
    };
    let lo = run.realised.keys().min().copied();
    let hi = run.realised.keys().max().copied();
    let (Some(lo), Some(hi)) = (lo, hi) else {
        return Ok(evaluation);
    };
    let mut hours = options.issuance_hours_utc.clone();
    hours.sort_unstable();
    hours.dedup();
    let mut day = lo.div_euclid(DAY_MS) * DAY_MS;
    while day <= hi {
        for &hour in &hours {
            let t_ms = day + i64::from(hour) * HOUR_MS;
            run.issue(t_ms, hour, &mut evaluation)?;
        }
        day += DAY_MS;
    }
    Ok(evaluation)
}

impl Run<'_> {
    fn issue(&self, t_ms: i64, hour: u8, out: &mut Evaluation) -> Result<(), EvalError> {
        let local = local_hour(instant(t_ms)?, &self.household);
        let record = |status| IssueRecord {
            t_ms,
            hour_utc: hour,
            local_hour: local,
            status,
        };
        let wind = self
            .data
            .vintages
            .get(&DATASET_WIND_FORECAST)
            .map_or(&[][..], Vec::as_slice);
        if select_vintages_as_of(wind, Some(t_ms)).is_empty() {
            out.issues.push(record(IssueStatus::PreVintage));
            return Ok(());
        }
        let inputs = reconstruct_issue_time(self.data, t_ms, AsOfMode::IssueTime)?;
        if !issue_has_grid_data(&inputs)? {
            out.issues.push(record(IssueStatus::NoGridData));
            return Ok(());
        }
        let mut cache = CorrectionCache::default();
        let forecasts = self
            .options
            .arms
            .iter()
            .map(|arm| match arm.kind {
                ArmKind::Model => forecast_at_issue_time_cached(&inputs, arm.variant, &mut cache),
                ArmKind::PureProfile => pure_profile_forecast(&inputs, arm.variant.shape_profile),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let dst_gap = self
            .options
            .arms
            .iter()
            .map(|arm| profile_at_issue_time(&inputs, arm.variant.shape_profile).map(|p| p.dst_gap))
            .collect::<Result<Vec<_>, _>>()?;
        let predictions: Vec<BTreeMap<i64, f64>> = forecasts
            .iter()
            .map(|fc| {
                fc.predicted
                    .iter()
                    .map(|(q, v)| (q.as_millisecond(), *v))
                    .collect()
            })
            .collect();
        let series_end_ms = forecasts
            .first()
            .map_or(inputs.origin_ms, |fc| fc.series_end_ms);
        let expected_quarters =
            usize::try_from((series_end_ms - inputs.origin_ms) / QUARTER_MS).unwrap_or(0);
        out.issues.push(record(IssueStatus::Forecast(ForecastIssue {
            leak_free: find_leaking_inputs(&inputs).is_empty(),
            fallback: forecasts.iter().map(|fc| fc.used_fallback).collect(),
            clamped_quarters: forecasts
                .iter()
                .map(|fc| fc.sanity_clamped_quarters)
                .collect(),
            expected_quarters,
            forecast_quarters: predictions.iter().map(BTreeMap::len).collect(),
            training_excluded: forecasts.iter().map(|fc| fc.training_excluded).collect(),
            correction: forecasts.iter().map(|fc| fc.correction).collect(),
            shape_fallback: forecasts
                .iter()
                .map(|fc| fc.shape_fallback_quarters)
                .collect(),
            dst_gap,
        })));
        self.targets(&inputs, hour, &predictions, out)?;
        self.tasks(&inputs, hour, &predictions, out)?;
        Ok(())
    }

    fn targets(
        &self,
        inputs: &IssueTimeInputs,
        hour: u8,
        predictions: &[BTreeMap<i64, f64>],
        out: &mut Evaluation,
    ) -> Result<(), EvalError> {
        let starts = |id| -> HashSet<i64> {
            inputs
                .forecast
                .get(&id)
                .into_iter()
                .flatten()
                .map(|v| v.start.as_millisecond())
                .collect()
        };
        let wind = starts(DATASET_WIND_FORECAST);
        let consumption = starts(DATASET_CONSUMPTION_FORECAST);
        let issue_day = date_in(inputs.t_ms, &self.delivery)?;
        let keys: BTreeSet<i64> = predictions.iter().flat_map(|p| p.keys().copied()).collect();
        for target_ms in keys {
            out.targets.push(TargetRecord {
                t_ms: inputs.t_ms,
                hour_utc: hour,
                day_ahead: days_between(issue_day, date_in(target_ms, &self.delivery)?)?,
                target_ms,
                actual: self.realised.get(&target_ms).copied(),
                predictions: predictions
                    .iter()
                    .map(|p| p.get(&target_ms).copied())
                    .collect(),
                last_week: baseline_for(Baseline::LastWeek, target_ms, inputs, &self.baseline),
                last_published_day: baseline_for(
                    Baseline::LastPublishedDay,
                    target_ms,
                    inputs,
                    &self.baseline,
                ),
                wind_vintage: wind.contains(&target_ms),
                consumption_vintage: consumption.contains(&target_ms),
            });
        }
        Ok(())
    }

    /// The task windows of one issuance: every within-day and across-day
    /// window that starts at or after the issue time and ends by the nominal
    /// series end. The nominal end follows the publication rule, not the
    /// stored data, so a missing price day shows up as an excluded task.
    fn windows(&self, t_ms: i64) -> Result<Vec<(TaskKind, i64, i64)>, EvalError> {
        let nominal_end = publication_horizon_ms(t_ms)? + FORECAST_DAYS * DAY_MS;
        let zone = &self.household;
        let mut windows = Vec::new();
        let mut date = date_in(t_ms, zone)?;
        let midnight_of = |d: Date| -> Result<i64, OutOfRange> {
            start_of_local_day_utc(d, zone)
                .map(Timestamp::as_millisecond)
                .map_err(|_| OutOfRange)
        };
        while midnight_of(date)? < nominal_end {
            let next = next_calendar_day(date).map_err(|_| OutOfRange)?;
            let midnight = midnight_of(date)?;
            let next_midnight = midnight_of(next)?;
            let noon = |d: Date| -> Result<i64, OutOfRange> {
                d.at(ACROSS_DAY_HOUR, 0, 0, 0)
                    .to_zoned(zone.clone())
                    .map(|z| z.timestamp().as_millisecond())
                    .map_err(|_| OutOfRange)
            };
            for (kind, start, end) in [
                (TaskKind::WithinDay, midnight, next_midnight),
                (TaskKind::AcrossDay, noon(date)?, noon(next)?),
            ] {
                if start >= t_ms && end <= nominal_end {
                    windows.push((kind, start, end));
                }
            }
            date = next;
        }
        Ok(windows)
    }

    fn tasks(
        &self,
        inputs: &IssueTimeInputs,
        hour: u8,
        predictions: &[BTreeMap<i64, f64>],
        out: &mut Evaluation,
    ) -> Result<(), EvalError> {
        for (kind, start, end) in self.windows(inputs.t_ms)? {
            for quarters in SCHEDULE_QUARTERS {
                let task = ScheduleTask {
                    earliest_start_ms: start,
                    deadline_ms: end,
                    quarters,
                };
                let window = task.window_quarters();
                let window_quarters = window.as_ref().map_or(0, Vec::len);
                let published_quarters = window
                    .as_ref()
                    .map_or(0, |w| w.iter().filter(|q| **q < inputs.origin_ms).count());
                out.tasks.push(TaskRecord {
                    t_ms: inputs.t_ms,
                    hour_utc: hour,
                    kind,
                    task,
                    published_quarters,
                    window_quarters,
                    outcome: schedule_arms(
                        inputs,
                        &task,
                        predictions,
                        &self.realised,
                        &self.pricer,
                        &self.baseline,
                    ),
                });
            }
        }
        Ok(())
    }
}

/// The realised mean cost (c/kWh) of every schedule arm on one task: the
/// model arms of `predictions`, then [`BASELINE_SCHEDULE_ARMS`]. The arms
/// share the published prices of `inputs` before its series start; after
/// it, each arm uses its own estimate. `realised` holds the FI spot prices.
/// `baseline_zone` is the zone of
/// [`baseline_zone`](crate::backtest::baseline_zone).
///
/// # Errors
/// The [`Exclusion`] of the task when an arm cannot choose or a run cannot
/// be scored. An excluded task is excluded for every arm.
pub fn schedule_arms(
    inputs: &IssueTimeInputs,
    task: &ScheduleTask,
    predictions: &[BTreeMap<i64, f64>],
    realised: &BTreeMap<i64, f64>,
    pricer: &Pricer,
    baseline_zone: &TimeZone,
) -> Result<Vec<f64>, Exclusion> {
    let window = task.window_quarters().map_err(|_| Exclusion::Infeasible)?;
    if task.deadline_ms <= inputs.origin_ms {
        return Err(Exclusion::FullyPublished);
    }
    if window.iter().any(|q| !realised.contains_key(q)) {
        return Err(Exclusion::MissingRealised);
    }
    let origin = inputs.origin_ms;
    let published = |q: i64| -> Option<f64> {
        inputs
            .fi_prices
            .get(&spot_price_domain::forecast::QuarterStart::from_millisecond(q))
            .copied()
    };
    let cost = |q: i64, spot: Option<f64>| spot.map(|s| pricer.price(s, q));
    let realised_cost = |q: i64| cost(q, realised.get(&q).copied());
    let choose = |estimate: &dyn Fn(i64) -> Option<f64>, missing: Exclusion| {
        cheapest_start(task, |q| {
            if q < origin {
                cost(q, published(q))
            } else {
                cost(q, estimate(q))
            }
        })
        .map_err(|e| match e {
            ScheduleError::Infeasible => Exclusion::Infeasible,
            ScheduleError::MissingQuarter(q) if q < origin => Exclusion::MissingRealised,
            ScheduleError::MissingQuarter(_) => missing,
        })
    };
    let mut starts = Vec::new();
    for (index, p) in predictions.iter().enumerate() {
        starts.push(choose(
            &|q| p.get(&q).copied(),
            Exclusion::MissingForecast(index),
        )?);
    }
    starts.push(choose(
        &|q| baseline_for(Baseline::LastWeek, q, inputs, baseline_zone),
        Exclusion::MissingLastWeek,
    )?);
    starts.push(choose(
        &|q| baseline_for(Baseline::LastPublishedDay, q, inputs, baseline_zone),
        Exclusion::MissingLastPublishedDay,
    )?);
    starts.push(task.earliest_start_ms);
    starts.push(cheapest_start(task, realised_cost).map_err(|_| Exclusion::MissingRealised)?);
    starts
        .into_iter()
        .map(|start| {
            run_mean_price(start, task.quarters, realised_cost)
                .map_err(|_| Exclusion::MissingRealised)
        })
        .collect()
}

/// A chronological split of the issuances by issue time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Split {
    pub selection_start_ms: i64,
    pub test_start_ms: i64,
}

/// A part of the issuances.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Segment {
    /// Every issuance; the run has no split.
    All,
    /// Before the selection start: for fitting.
    Train,
    /// From the selection start to the test start: for model selection.
    Selection,
    /// From the test start: the final test.
    Test,
}

impl Segment {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Train => "train",
            Self::Selection => "selection",
            Self::Test => "test",
        }
    }

    /// Whether the issuance at `t_ms` is in this segment of `split`.
    #[must_use]
    pub fn contains(self, t_ms: i64, split: Option<Split>) -> bool {
        match (self, split) {
            (Self::All, _) => true,
            (_, None) => false,
            (Self::Train, Some(s)) => t_ms < s.selection_start_ms,
            (Self::Selection, Some(s)) => (s.selection_start_ms..s.test_start_ms).contains(&t_ms),
            (Self::Test, Some(s)) => t_ms >= s.test_start_ms,
        }
    }

    /// The start of the next segment; `None` for the last segment.
    #[must_use]
    pub const fn end_ms(self, split: Option<Split>) -> Option<i64> {
        match (self, split) {
            (Self::Train, Some(s)) => Some(s.selection_start_ms),
            (Self::Selection, Some(s)) => Some(s.test_start_ms),
            _ => None,
        }
    }

    /// Whether a target of an issuance in this segment is delivered before
    /// the next segment starts. A later target belongs to a later segment's
    /// period, so this segment never scores it.
    #[must_use]
    pub fn holds_target(self, target: &TargetRecord, split: Option<Split>) -> bool {
        self.contains(target.t_ms, split)
            && self.end_ms(split).is_none_or(|end| target.target_ms < end)
    }

    /// Whether a task of an issuance in this segment ends by the start of
    /// the next segment. See [`Segment::holds_target`].
    #[must_use]
    pub fn holds_task(self, task: &TaskRecord, split: Option<Split>) -> bool {
        self.contains(task.t_ms, split)
            && self
                .end_ms(split)
                .is_none_or(|end| task.task.deadline_ms <= end)
    }
}

/// One named statistic of a segment. Criteria refer to the `id`.
#[derive(Debug, Clone, PartialEq)]
pub struct Statistic {
    pub id: String,
    /// The paired targets or tasks behind the value.
    pub n: usize,
    pub estimate: Estimate,
}

/// The per-day sums of one statistic, keyed by the UTC issuance day.
#[derive(Debug, Default)]
struct DaySums {
    by_day: BTreeMap<i64, UnitSums>,
    n: usize,
}

impl DaySums {
    fn add(&mut self, t_ms: i64, numerator: f64, denominator: f64) {
        let unit = self.by_day.entry(t_ms.div_euclid(DAY_MS)).or_default();
        unit.numerator += numerator;
        unit.denominator += denominator;
        self.n += 1;
    }
}

/// The horizon label of a delivery-day offset.
#[must_use]
pub fn day_ahead_label(day_ahead: i64) -> String {
    format!("D+{day_ahead}")
}

/// The label of a run length in quarters.
#[must_use]
pub fn duration_label(quarters: usize) -> String {
    format!("{}h", quarters / 4)
}

/// The day sums of every statistic id of one segment.
#[derive(Debug, Default)]
struct Sums(BTreeMap<String, DaySums>);

impl Sums {
    fn add(&mut self, id: String, t_ms: i64, numerator: f64, denominator: f64) {
        self.0
            .entry(id)
            .or_default()
            .add(t_ms, numerator, denominator);
    }

    /// MAE, rMAE, and arm deltas of the paired targets.
    fn add_target(&mut self, arms: &[String], paired: &[(usize, usize)], target: &TargetRecord) {
        let (Some(actual), Some(lw), Some(lpd)) =
            (target.actual, target.last_week, target.last_published_day)
        else {
            return;
        };
        if !target.complete() {
            return;
        }
        let key = format!(
            "{:02}/{}",
            target.hour_utc,
            day_ahead_label(target.day_ahead)
        );
        let t = target.t_ms;
        let errors: Vec<f64> = target
            .predictions
            .iter()
            .map(|p| (p.unwrap_or(f64::NAN) - actual).abs())
            .collect();
        let (lw_error, lpd_error) = ((lw - actual).abs(), (lpd - actual).abs());
        self.add(format!("mae/last_week/{key}"), t, lw_error, 1.0);
        self.add(format!("mae/last_published_day/{key}"), t, lpd_error, 1.0);
        for (name, error) in arms.iter().zip(&errors) {
            self.add(format!("mae/{name}/{key}"), t, *error, 1.0);
            self.add(format!("rmae/{name}/last_week/{key}"), t, *error, lw_error);
            self.add(
                format!("rmae/{name}/last_published_day/{key}"),
                t,
                *error,
                lpd_error,
            );
        }
        if let (Some(reference), Some(reference_error)) = (arms.first(), errors.first()) {
            for (name, error) in arms.iter().zip(&errors).skip(1) {
                self.add(
                    format!("mae_delta/{name}-{reference}/{key}"),
                    t,
                    error - reference_error,
                    1.0,
                );
            }
        }
        for &(arm, reference) in paired {
            let (Some(name), Some(other), Some(error), Some(other_error)) = (
                arms.get(arm),
                arms.get(reference),
                errors.get(arm),
                errors.get(reference),
            ) else {
                continue;
            };
            self.add(
                format!("mae_delta/{name}-{other}/{key}"),
                t,
                error - other_error,
                1.0,
            );
        }
    }

    /// Costs, regrets, and cost deltas of one scored task.
    fn add_task(
        &mut self,
        arms: &[String],
        schedule_arms: &[String],
        paired: &[(usize, usize)],
        task: &TaskRecord,
    ) {
        let Ok(costs) = &task.outcome else { return };
        let hindsight = schedule_arms.len().saturating_sub(1);
        let key = format!(
            "{}/{}/{:02}",
            task.kind.label(),
            duration_label(task.task.quarters),
            task.hour_utc
        );
        let t = task.t_ms;
        let best = costs.get(hindsight).copied().unwrap_or(f64::NAN);
        for (name, cost) in schedule_arms.iter().zip(costs) {
            self.add(format!("cost/{name}/{key}"), t, *cost, 1.0);
            self.add(format!("regret/{name}/{key}"), t, cost - best, 1.0);
        }
        for (m, name) in arms.iter().enumerate() {
            let model = costs.get(m).copied().unwrap_or(f64::NAN);
            // Each model arm against the price-blind and persistence arms,
            // and every later model arm against the first one.
            let mut others: Vec<usize> = (arms.len()..hindsight).collect();
            if m > 0 {
                others.insert(0, 0);
            }
            for o in others {
                let (Some(other), Some(value)) = (schedule_arms.get(o), costs.get(o)) else {
                    continue;
                };
                self.add(
                    format!("cost_delta/{name}-{other}/{key}"),
                    t,
                    model - value,
                    1.0,
                );
            }
        }
        for &(arm, reference) in paired {
            let (Some(name), Some(other), Some(cost), Some(other_cost)) = (
                arms.get(arm),
                arms.get(reference),
                costs.get(arm),
                costs.get(reference),
            ) else {
                continue;
            };
            self.add(
                format!("cost_delta/{name}-{other}/{key}"),
                t,
                cost - other_cost,
                1.0,
            );
        }
    }
}

/// Every statistic of the issuances in `segment`, with its interval. All
/// statistics use the same resampled days. A target or task that crosses
/// into the next segment's period is left out.
#[must_use]
pub fn statistics(
    evaluation: &Evaluation,
    segment: Segment,
    split: Option<Split>,
    bootstrap: &BootstrapOptions,
) -> Vec<Statistic> {
    let mut sums = Sums::default();
    let arms = &evaluation.arm_names;
    for target in &evaluation.targets {
        if segment.holds_target(target, split) {
            sums.add_target(arms, &evaluation.paired, target);
        }
    }
    let schedule_arms = evaluation.schedule_arm_names();
    for task in &evaluation.tasks {
        if segment.holds_task(task, split) {
            sums.add_task(arms, &schedule_arms, &evaluation.paired, task);
        }
    }
    let days: BTreeSet<i64> = evaluation
        .issues
        .iter()
        .filter(|i| segment.contains(i.t_ms, split))
        .map(|i| i.t_ms.div_euclid(DAY_MS))
        .collect();
    let (Some(first), Some(last)) = (days.first().copied(), days.last().copied()) else {
        return Vec::new();
    };
    let units = usize::try_from(last - first + 1).unwrap_or(0);
    let draws = resample_indices(units, bootstrap);
    sums.0
        .into_iter()
        .map(|(id, s)| {
            let mut unit_sums = vec![UnitSums::default(); units];
            for (day, sum) in s.by_day {
                if let Some(slot) = usize::try_from(day - first)
                    .ok()
                    .and_then(|i| unit_sums.get_mut(i))
                {
                    *slot = sum;
                }
            }
            Statistic {
                id,
                n: s.n,
                estimate: estimate(&unit_sums, &draws),
            }
        })
        .collect()
}

/// FNV-1a (64-bit) of `text`, as 16 hex digits. It names an input snapshot;
/// it is not a security hash.
#[must_use]
pub fn fnv1a_hex(text: &str) -> String {
    let mut hash: u64 = 0xCBF2_9CE4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01B3);
    }
    format!("{hash:016x}")
}
