//! `spot-price-tools backtest --eval`: the evaluation report of the common
//! evaluation contract. It reads the same `--data` fixture or `--db` window
//! as the backtest. The older backtest report stays unchanged; its numbers
//! keep their own version and are not comparable row by row.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use jiff::civil::Date;
use jiff::tz::TimeZone;
use spot_price_domain::js::string_to_number;
use spot_price_domain::settings::Hour;

use crate::backtest::{DELIVERY_DAY_TIME_ZONE, ForecastVariant, PUBLICATION_HOUR_UTC};
use crate::backtest_cli::{MIN_SCOREABLE_ORIGINS, fmt, iso_or_na, parse_variant, variant_label};
use crate::bootstrap::{BootstrapOptions, DEFAULT_BOOTSTRAP, MIN_OVERLAP_BLOCK_DAYS};
use crate::cli::{Output, flag_value, has_flag};
use crate::criteria::{DeclaredCriteria, Outcome, Verdict, load_criteria};
use crate::data::{BacktestData, to_fixture_json};
use crate::error::ToolError;
use crate::evaluation::{
    DEFAULT_ISSUANCE_HOURS_UTC, EVAL_VERSION, EvalOptions, Evaluation, Exclusion, IssueStatus,
    ModelArm, POWER_KW, SCHEDULE_QUARTERS, Segment, Split, Statistic, TaskKind, day_ahead_label,
    duration_label, evaluate, fnv1a_hex, statistics,
};
use crate::metrics::{median, percentile};
use crate::schedule::{ContractScenario, CostBasis, HOUSEHOLD_TIME_ZONE};

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// The flags of an evaluation run.
#[derive(Debug, Clone, PartialEq)]
pub struct EvalFlags {
    pub options: EvalOptions,
    pub split: Option<Split>,
    pub bootstrap: BootstrapOptions,
    pub criteria: Option<DeclaredCriteria>,
}

fn argument(message: String) -> ToolError {
    ToolError::Argument(message)
}

fn whole_number(flag: &str, raw: &str, max: f64) -> Result<f64, ToolError> {
    let value = string_to_number(raw);
    if raw.is_empty() || !value.is_finite() || value.fract() != 0.0 || value < 0.0 || value > max {
        return Err(argument(format!(
            "{flag} must be a whole number from 0 to {max}, got \"{raw}\""
        )));
    }
    Ok(value)
}

/// `--issuances <h[,h]>`: distinct UTC hours 0–23.
///
/// # Errors
/// [`ToolError::Argument`] for an empty list, a bad hour, or a repeat.
pub fn parse_issuances(raw: &str) -> Result<Vec<u8>, ToolError> {
    let mut hours = Vec::new();
    for item in raw.split(',') {
        let value = whole_number("--issuances", item.trim(), 23.0)?;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "value is a whole number in 0..=23"
        )]
        let hour = value as u8;
        if hours.contains(&hour) {
            return Err(argument(format!("--issuances repeats hour {hour}")));
        }
        hours.push(hour);
    }
    hours.sort_unstable();
    Ok(hours)
}

fn finite(key: &str, raw: &str) -> Result<f64, ToolError> {
    let value = string_to_number(raw);
    if raw.is_empty() || !value.is_finite() {
        return Err(argument(format!(
            "--cost contract {key} must be a finite number, got \"{raw}\""
        )));
    }
    Ok(value)
}

fn hour(key: &str, raw: &str) -> Result<Hour, ToolError> {
    let value = whole_number(&format!("--cost contract {key}"), raw, 23.0)?;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "value is a whole number in 0..=23"
    )]
    Hour::new(value as u8).map_err(|_| argument(format!("--cost contract {key} is not an hour")))
}

/// The keys of a contract scenario, all required.
const CONTRACT_KEYS: [&str; 7] = [
    "margin",
    "day",
    "night",
    "tax",
    "vat",
    "night_start",
    "night_end",
];

/// `--cost spot` or `--cost contract:margin=…,day=…,night=…,tax=…,vat=…,
/// night_start=…,night_end=…` (c/kWh, VAT in percent, local hours).
///
/// # Errors
/// [`ToolError::Argument`] for another value, a missing or repeated key, or
/// a bad number.
pub fn parse_cost(raw: &str) -> Result<CostBasis, ToolError> {
    if raw == "spot" {
        return Ok(CostBasis::Spot);
    }
    let Some(list) = raw.strip_prefix("contract:") else {
        return Err(argument(format!(
            "--cost must be spot or contract:<key=value,...>, got \"{raw}\""
        )));
    };
    let mut values: BTreeMap<&str, &str> = BTreeMap::new();
    for item in list.split(',') {
        let Some((key, value)) = item.split_once('=') else {
            return Err(argument(format!(
                "--cost contract expects key=value pairs, got \"{item}\""
            )));
        };
        let key = key.trim();
        if !CONTRACT_KEYS.contains(&key) {
            return Err(argument(format!(
                "--cost contract key must be one of {}, got \"{key}\"",
                CONTRACT_KEYS.join(", ")
            )));
        }
        if values.insert(key, value.trim()).is_some() {
            return Err(argument(format!(
                "--cost contract sets {key} more than once"
            )));
        }
    }
    let get = |key: &str| {
        values
            .get(key)
            .copied()
            .ok_or_else(|| argument(format!("--cost contract needs {key}")))
    };
    Ok(CostBasis::Contract(ContractScenario {
        margin_cents_kwh: finite("margin", get("margin")?)?,
        transfer_day_cents_kwh: finite("day", get("day")?)?,
        transfer_night_cents_kwh: finite("night", get("night")?)?,
        tax_cents_kwh: finite("tax", get("tax")?)?,
        vat_percent: finite("vat", get("vat")?)?,
        night_start_hour: hour("night_start", get("night_start")?)?,
        night_end_hour: hour("night_end", get("night_end")?)?,
    }))
}

fn utc_midnight(flag: &str, raw: &str) -> Result<i64, ToolError> {
    raw.parse::<Date>()
        .ok()
        .and_then(|d| d.to_zoned(TimeZone::UTC).ok())
        .map(|z| z.timestamp().as_millisecond())
        .ok_or_else(|| argument(format!("{flag} expects YYYY-MM-DD dates, got \"{raw}\"")))
}

/// `--split <selection-start>,<test-start>`: two UTC dates in order.
///
/// # Errors
/// [`ToolError::Argument`] for a bad date or dates out of order.
pub fn parse_split(raw: &str) -> Result<Split, ToolError> {
    let Some((selection, test)) = raw.split_once(',') else {
        return Err(argument(format!(
            "--split expects <selection-start>,<test-start>, got \"{raw}\""
        )));
    };
    let split = Split {
        selection_start_ms: utc_midnight("--split", selection.trim())?,
        test_start_ms: utc_midnight("--split", test.trim())?,
    };
    if split.selection_start_ms >= split.test_start_ms {
        return Err(argument(
            "--split selection start must be before the test start".to_owned(),
        ));
    }
    Ok(split)
}

fn bootstrap_flags(args: &[String]) -> Result<BootstrapOptions, ToolError> {
    let mut options = DEFAULT_BOOTSTRAP;
    if let Some(raw) = flag_value(args, "--seed") {
        // Up to 2^53, the exact integer range of a JavaScript number.
        let seed = whole_number("--seed", raw, 9_007_199_254_740_991.0)?;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "seed is a whole number in 0..=2^53"
        )]
        let seed = seed as u64;
        options.seed = seed;
    }
    if let Some(raw) = flag_value(args, "--resamples") {
        let n = whole_number("--resamples", raw, 100_000.0)?;
        if n < 1.0 {
            return Err(argument("--resamples must be at least 1".to_owned()));
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "n is a whole number in 1..=100000"
        )]
        let n = n as usize;
        options.resamples = n;
    }
    if let Some(raw) = flag_value(args, "--block-days") {
        let n = whole_number("--block-days", raw, 366.0)?;
        if n < 1.0 {
            return Err(argument("--block-days must be at least 1".to_owned()));
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "n is a whole number in 1..=366"
        )]
        let n = n as usize;
        options.block_days = n;
    }
    Ok(options)
}

/// Reads the evaluation flags. Every flag is checked before any data is
/// read.
///
/// # Errors
/// [`ToolError`] for a bad flag or an unreadable criteria file.
pub fn parse_flags(args: &[String]) -> Result<EvalFlags, ToolError> {
    let value = |flag: &str| -> Result<Option<&str>, ToolError> {
        if !has_flag(args, flag) {
            return Ok(None);
        }
        flag_value(args, flag)
            .map(Some)
            .ok_or_else(|| argument(format!("{flag} needs a value")))
    };
    let issuance_hours_utc = match value("--issuances")? {
        Some(raw) => parse_issuances(raw)?,
        None => DEFAULT_ISSUANCE_HOURS_UTC.to_vec(),
    };
    let cost = value("--cost")?.map_or(Ok(CostBasis::Spot), parse_cost)?;
    let mut arms = vec![ModelArm {
        name: "A".to_owned(),
        variant: ForecastVariant::default(),
    }];
    if let Some(raw) = value("--ab")? {
        arms.push(ModelArm {
            name: "B".to_owned(),
            variant: parse_variant(raw)?,
        });
    }
    let split = value("--split")?.map(parse_split).transpose()?;
    let criteria = value("--criteria")?
        .map(|file| load_criteria(Path::new(file)))
        .transpose()?;
    Ok(EvalFlags {
        options: EvalOptions {
            issuance_hours_utc,
            arms,
            cost,
        },
        split,
        bootstrap: bootstrap_flags(args)?,
        criteria,
    })
}

fn hh(hour: u8) -> String {
    format!("{hour:02}")
}

fn cost_label(cost: &CostBasis) -> String {
    match cost {
        CostBasis::Spot => "spot price (c/kWh), negative prices kept".to_owned(),
        CostBasis::Contract(c) => format!(
            "declared contract scenario margin={} day={} night={} tax={} vat={}% night={:02}–{:02} ({HOUSEHOLD_TIME_ZONE}); the API math floors a negative spot at 0. Not a historical user setting.",
            c.margin_cents_kwh,
            c.transfer_day_cents_kwh,
            c.transfer_night_cents_kwh,
            c.tax_cents_kwh,
            c.vat_percent,
            c.night_start_hour.get(),
            c.night_end_hour.get(),
        ),
    }
}

fn split_label(split: Option<Split>) -> String {
    split.map_or_else(
        || "none: every issuance is one segment".to_owned(),
        |s| {
            format!(
                "train < {} <= selection < {} <= test (issue time, UTC)",
                iso_or_na(Some(s.selection_start_ms)),
                iso_or_na(Some(s.test_start_ms))
            )
        },
    )
}

fn print_header(data: &BacktestData, flags: &EvalFlags, out: &mut Output) {
    let options = &flags.options;
    out.log(&format!(
        "\nFI forecast evaluation ({EVAL_VERSION}, spot-price-tools {})",
        env!("CARGO_PKG_VERSION")
    ));
    out.log(&format!(
        "  input snapshot:      {} prices, FNV-1a 64 {} of the --export fixture text",
        data.prices.len(),
        fnv1a_hex(&to_fixture_json(data))
    ));
    out.log(&format!(
        "  issuances (UTC):     {}",
        options
            .issuance_hours_utc
            .iter()
            .map(|h| format!("{}:00", hh(*h)))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    for arm in &options.arms {
        out.log(&format!(
            "  arm {}:               {}",
            arm.name,
            variant_label(arm.variant)
        ));
    }
    out.log(&format!(
        "  cost basis:          {}",
        cost_label(&options.cost)
    ));
    out.log(&format!(
        "  schedules:           one continuous run of {} at {POWER_KW} kW per task; within_day 00:00–24:00, across_day 12:00–12:00 next day ({HOUSEHOLD_TIME_ZONE}); same window, run length, energy, and power for every arm",
        SCHEDULE_QUARTERS
            .iter()
            .map(|q| duration_label(*q))
            .collect::<Vec<_>>()
            .join(" and ")
    ));
    out.log(&format!(
        "  bootstrap:           circular blocks of {} issuance days, {} resamples, seed {}, 95 % percentile interval; a day keeps all its issuances",
        flags.bootstrap.block_days, flags.bootstrap.resamples, flags.bootstrap.seed
    ));
    out.log(&format!(
        "  split:               {}",
        split_label(flags.split)
    ));

    out.log("\n  RECONSTRUCTION ASSUMPTIONS (labelled, not measured):");
    out.log(&format!(
        "   - A1 publication: delivery day D counts as published from {PUBLICATION_HOUR_UTC}:00 UTC on D−1 ({DELIVERY_DAY_TIME_ZONE} delivery day). Stored rows carry no publication time. A late Nord Pool publication after the issue time is not detected."
    ));
    out.log("   - A2 grid forecasts: a 245/165 vintage counts from its stored issued_at, the hour-truncated fetch time, strictly before the issue time. This is up to one hour staler than the live service. Coverage starts with the vintage archive and ends at its retention.");
    out.log("   - A3 grid actuals: 75/124 rows are the latest stored revision, used for quarters strictly before the issue time. The real publication delay and earlier revisions are not reconstructed.");
    out.log("   - A4 neighbour prices (SE1, SE3, EE) follow the FI publication rule A1.");
    out.log(&format!(
        "   - A5 horizons: D+k is the {DELIVERY_DAY_TIME_ZONE} delivery day of the target minus the delivery day of the issue time. Schedule windows and local issue hours are in {HOUSEHOLD_TIME_ZONE}."
    ));
    out.log("   - A6 forecast error is scored only on targets after the last published quarter; every schedule arm gets the same published prices.");
}

fn status_counts(
    evaluation: &Evaluation,
    segment: Segment,
    split: Option<Split>,
    out: &mut Output,
) {
    let arms = evaluation.arm_names.join("/");
    out.log("\n  availability per issuance hour:");
    out.log(&format!(
        "    {:<5}  {:<12}  {:>10}  {:>10}  {:>10}  {:>9}  {:>8}  {:>14}  {:>14}  {:>22}",
        "hour",
        "local hours",
        "candidates",
        "preVintage",
        "noGridData",
        "forecasts",
        "leakFree",
        format!("fallback {arms}"),
        format!("clamped {arms}"),
        format!("missing outputs {arms}"),
    ));
    for &hour in &evaluation.issuance_hours_utc {
        let issues: Vec<_> = evaluation
            .issues
            .iter()
            .filter(|i| i.hour_utc == hour && segment.contains(i.t_ms, split))
            .collect();
        let locals: BTreeSet<u8> = issues.iter().map(|i| i.local_hour).collect();
        let count = |f: fn(&IssueStatus) -> bool| issues.iter().filter(|i| f(&i.status)).count();
        let forecasts: Vec<_> = issues
            .iter()
            .filter_map(|i| match &i.status {
                IssueStatus::Forecast(f) => Some(f),
                _ => None,
            })
            .collect();
        let per_arm = |f: &dyn Fn(&crate::evaluation::ForecastIssue, usize) -> usize| {
            (0..evaluation.arm_names.len())
                .map(|a| forecasts.iter().map(|x| f(x, a)).sum::<usize>().to_string())
                .collect::<Vec<_>>()
                .join("/")
        };
        out.log(&format!(
            "    {:<5}  {:<12}  {:>10}  {:>10}  {:>10}  {:>9}  {:>8}  {:>14}  {:>14}  {:>22}",
            hh(hour),
            locals.iter().map(|h| hh(*h)).collect::<Vec<_>>().join(","),
            issues.len(),
            count(|s| matches!(s, IssueStatus::PreVintage)),
            count(|s| matches!(s, IssueStatus::NoGridData)),
            forecasts.len(),
            forecasts.iter().all(|f| f.leak_free),
            per_arm(&|f, a| usize::from(f.fallback.get(a).copied().unwrap_or(false))),
            per_arm(&|f, a| f.clamped_quarters.get(a).copied().unwrap_or(0)),
            per_arm(&|f, a| f
                .expected_quarters
                .saturating_sub(f.forecast_quarters.get(a).copied().unwrap_or(0))),
        ));
    }
}

fn target_tables(
    evaluation: &Evaluation,
    segment: Segment,
    split: Option<Split>,
    out: &mut Output,
) {
    out.log("\n  targets, lead hours, and inputs (paired = realised price, every arm, and both baselines):");
    out.log(&format!(
        "    {:<5}  {:<7}  {:>7}  {:>6}  {:>9}  {:>18}  {:>8}  {:>8}  {:>10}  {:>8}  {:>8}  {:>8}",
        "hour",
        "horizon",
        "targets",
        "paired",
        "issuances",
        "lead h min/med/max",
        "wind245",
        "cons165",
        "noRealised",
        "noArm",
        "noLastWk",
        "noLastPub"
    ));
    let mut groups: BTreeMap<(u8, i64), Vec<&crate::evaluation::TargetRecord>> = BTreeMap::new();
    for t in evaluation
        .targets
        .iter()
        .filter(|t| segment.contains(t.t_ms, split))
    {
        groups.entry((t.hour_utc, t.day_ahead)).or_default().push(t);
    }
    for ((hour, day_ahead), targets) in groups {
        let paired: Vec<_> = targets.iter().filter(|t| t.complete()).collect();
        let leads: Vec<f64> = paired.iter().map(|t| t.lead_hours()).collect();
        let issuances: BTreeSet<i64> = paired.iter().map(|t| t.t_ms).collect();
        let share = |f: fn(&crate::evaluation::TargetRecord) -> bool| {
            #[expect(clippy::cast_precision_loss, reason = "small counts")]
            let value = (!paired.is_empty())
                .then(|| paired.iter().filter(|t| f(t)).count() as f64 / paired.len() as f64);
            fmt(value, 3)
        };
        let count = |f: fn(&crate::evaluation::TargetRecord) -> bool| {
            targets.iter().filter(|t| f(t)).count()
        };
        out.log(&format!(
            "    {:<5}  {:<7}  {:>7}  {:>6}  {:>9}  {:>18}  {:>8}  {:>8}  {:>10}  {:>8}  {:>8}  {:>8}",
            hh(hour),
            day_ahead_label(day_ahead),
            targets.len(),
            paired.len(),
            issuances.len(),
            format!(
                "{}/{}/{}",
                fmt(percentile(&leads, 0.0), 1),
                fmt(median(&leads), 1),
                fmt(percentile(&leads, 100.0), 1)
            ),
            share(|t| t.wind_vintage),
            share(|t| t.consumption_vintage),
            count(|t| t.actual.is_none()),
            count(|t| t.predictions.iter().any(Option::is_none)),
            count(|t| t.last_week.is_none()),
            count(|t| t.last_published_day.is_none()),
        ));
    }
    out.log("    noRealised counts targets after the last stored price or in a price gap. The last issuances of a window have no stored next day, so their series start early and their targets are not paired.");
}

fn stat_map(stats: &[Statistic]) -> BTreeMap<&str, &Statistic> {
    stats.iter().map(|s| (s.id.as_str(), s)).collect()
}

fn point(stats: &BTreeMap<&str, &Statistic>, id: &str) -> String {
    fmt(stats.get(id).and_then(|s| s.estimate.point), 3)
}

fn with_interval(stats: &BTreeMap<&str, &Statistic>, id: &str) -> String {
    stats.get(id).map_or_else(
        || "n/a".to_owned(),
        |s| {
            let e = &s.estimate;
            match e.point {
                None => "undefined".to_owned(),
                Some(_) => format!(
                    "{} [{}, {}]",
                    fmt(e.point, 3),
                    fmt(e.low, 3),
                    fmt(e.high, 3)
                ),
            }
        },
    )
}

fn accuracy_table(evaluation: &Evaluation, stats: &[Statistic], out: &mut Output) {
    let map = stat_map(stats);
    let keys: BTreeSet<String> = stats
        .iter()
        .filter_map(|s| s.id.strip_prefix("mae/last_week/").map(str::to_owned))
        .collect();
    out.log("\n  accuracy on paired targets (MAE in c/kWh; rMAE = arm MAE / baseline MAE, undefined when the baseline MAE is 0; [95 % interval]):");
    for key in keys {
        let n = map
            .get(format!("mae/last_week/{key}").as_str())
            .map_or(0, |s| s.n);
        out.log(&format!(
            "    {key:<9}  paired {n:>6}  MAE last_week {}  last_published_day {}",
            point(&map, &format!("mae/last_week/{key}")),
            point(&map, &format!("mae/last_published_day/{key}"))
        ));
        for name in &evaluation.arm_names {
            out.log(&format!(
                "      arm {name}: MAE {}  rMAE last_week {}  rMAE last_published_day {}",
                with_interval(&map, &format!("mae/{name}/{key}")),
                with_interval(&map, &format!("rmae/{name}/last_week/{key}")),
                with_interval(&map, &format!("rmae/{name}/last_published_day/{key}")),
            ));
        }
        if let Some(reference) = evaluation.arm_names.first() {
            for name in evaluation.arm_names.iter().skip(1) {
                out.log(&format!(
                    "      paired MAE delta {name} − {reference}: {}",
                    with_interval(&map, &format!("mae_delta/{name}-{reference}/{key}"))
                ));
            }
        }
    }
}

const fn exclusion_label(e: Exclusion) -> &'static str {
    match e {
        Exclusion::FullyPublished => "fully published",
        Exclusion::Infeasible => "infeasible window",
        Exclusion::MissingRealised => "missing realised price",
        Exclusion::MissingForecast(_) => "missing forecast output",
        Exclusion::MissingLastWeek => "missing last_week value",
        Exclusion::MissingLastPublishedDay => "missing last_published_day value",
    }
}

fn schedule_tables(
    evaluation: &Evaluation,
    stats: &[Statistic],
    segment: Segment,
    split: Option<Split>,
    out: &mut Output,
) {
    let map = stat_map(stats);
    let arms = evaluation.schedule_arm_names();
    out.log("\n  schedules (realised mean cost in c/kWh of the run; regret = cost − hindsight cost; [95 % interval]):");
    for &hour in &evaluation.issuance_hours_utc {
        for kind in TaskKind::ALL {
            for quarters in SCHEDULE_QUARTERS {
                let tasks: Vec<_> = evaluation
                    .tasks
                    .iter()
                    .filter(|t| {
                        t.hour_utc == hour
                            && t.kind == kind
                            && t.task.quarters == quarters
                            && segment.contains(t.t_ms, split)
                    })
                    .collect();
                if tasks.is_empty() {
                    continue;
                }
                let scored: Vec<_> = tasks.iter().filter(|t| t.outcome.is_ok()).collect();
                let leads: Vec<f64> = scored.iter().map(|t| t.lead_hours()).collect();
                #[expect(clippy::cast_precision_loss, reason = "small counts")]
                let published: Vec<f64> = scored
                    .iter()
                    .map(|t| t.published_quarters as f64 / t.window_quarters.max(1) as f64)
                    .collect();
                let mut exclusions: BTreeMap<&str, usize> = BTreeMap::new();
                for t in &tasks {
                    if let Err(e) = t.outcome {
                        *exclusions.entry(exclusion_label(e)).or_default() += 1;
                    }
                }
                let key = format!("{}/{}/{}", kind.label(), duration_label(quarters), hh(hour));
                out.log(&format!(
                    "    {key:<19}  tasks {}  scored {}  lead h median {}  published share mean {}  excluded: {}",
                    tasks.len(),
                    scored.len(),
                    fmt(median(&leads), 1),
                    fmt(
                        (!published.is_empty()).then(|| crate::jsfmt::js_sum(published.iter().copied())
                            / crate::jsfmt::count_f64(published.len())),
                        3
                    ),
                    if exclusions.is_empty() {
                        "none".to_owned()
                    } else {
                        exclusions
                            .iter()
                            .map(|(k, v)| format!("{k} {v}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                ));
                if scored.is_empty() {
                    continue;
                }
                for arm in &arms {
                    out.log(&format!(
                        "      {arm:<19} cost {}  regret {}",
                        point(&map, &format!("cost/{arm}/{key}")),
                        with_interval(&map, &format!("regret/{arm}/{key}")),
                    ));
                }
                for s in stats.iter().filter(|s| {
                    s.id.starts_with("cost_delta/") && s.id.ends_with(&format!("/{key}"))
                }) {
                    out.log(&format!("      {}: {}", s.id, with_interval(&map, &s.id)));
                }
            }
        }
    }
}

fn statistics_table(stats: &[Statistic], out: &mut Output) {
    out.log("\n  statistics (criteria refer to these ids):");
    out.log(&format!(
        "    {:<52}  {:>6}  {:>9}  {:>9}  {:>9}  {:>9}",
        "id", "n", "point", "ci_low", "ci_high", "undefined"
    ));
    for s in stats {
        let e = &s.estimate;
        out.log(&format!(
            "    {:<52}  {:>6}  {:>9}  {:>9}  {:>9}  {:>9}",
            s.id,
            s.n,
            fmt(e.point, 4),
            fmt(e.low, 4),
            fmt(e.high, 4),
            e.undefined_resamples
        ));
    }
}

fn print_segment(
    evaluation: &Evaluation,
    flags: &EvalFlags,
    segment: Segment,
    stats: &[Statistic],
    out: &mut Output,
) {
    let days: BTreeSet<i64> = evaluation
        .issues
        .iter()
        .filter(|i| {
            matches!(i.status, IssueStatus::Forecast(_)) && segment.contains(i.t_ms, flags.split)
        })
        .map(|i| i.t_ms.div_euclid(DAY_MS) * DAY_MS)
        .collect();
    #[expect(clippy::cast_precision_loss, reason = "a day count is small")]
    let span = match (days.first(), days.last()) {
        (Some(a), Some(b)) => (b - a) as f64 / DAY_MS as f64 + 1.0,
        _ => 0.0,
    };
    out.log(&format!(
        "\n== segment: {}  issuance days {} … {} ({} days with a forecast, span {} days)",
        segment.label(),
        iso_or_na(days.first().copied()),
        iso_or_na(days.last().copied()),
        days.len(),
        fmt(Some(span), 0)
    ));
    status_counts(evaluation, segment, flags.split, out);
    target_tables(evaluation, segment, flags.split, out);
    accuracy_table(evaluation, stats, out);
    schedule_tables(evaluation, stats, segment, flags.split, out);
    statistics_table(stats, out);
}

fn decided(value: Option<bool>, yes: &str, no: &str) -> String {
    match value {
        Some(true) => yes.to_owned(),
        Some(false) => no.to_owned(),
        None => "UNDETERMINED (an outcome is undefined or missing)".to_owned(),
    }
}

fn print_criteria(
    declared: &DeclaredCriteria,
    segment: Segment,
    stats: &[Statistic],
    out: &mut Output,
) {
    let c = &declared.criteria;
    let verdict = Verdict::of(c, stats);
    out.log(&format!(
        "\n== criteria on segment {}: \"{}\" declared {} (file FNV-1a 64 {})",
        segment.label(),
        c.experiment,
        c.declared,
        declared.digest
    ));
    let show = |label: &str,
                list: &[crate::criteria::Criterion],
                outcomes: &[Outcome],
                out: &mut Output| {
        for (criterion, outcome) in list.iter().zip(outcomes) {
            let value = match outcome {
                Outcome::Met(v) | Outcome::NotMet(v) => fmt(Some(*v), 4),
                _ => "n/a".to_owned(),
            };
            out.log(&format!(
                "    {label:<10} {} {} {} {}  → {} (value {value})",
                criterion.id,
                criterion.statistic.label(),
                criterion.op.label(),
                criterion.value,
                outcome.label()
            ));
        }
    };
    show("success", &c.success, &verdict.success, out);
    show("regression", &c.regression, &verdict.regression, out);
    out.log(&format!(
        "  success:    {}",
        if c.success.is_empty() {
            "none declared".to_owned()
        } else {
            decided(verdict.success_met(), "MET", "NOT MET")
        }
    ));
    out.log(&format!(
        "  regression: {}",
        if c.regression.is_empty() {
            "none declared".to_owned()
        } else {
            decided(verdict.regression_detected(), "DETECTED", "none detected")
        }
    ));
    out.log("  The tool reports outcomes only. The owner records adopt, reject, or defer.");
}

/// Runs the evaluation and prints the report; returns false for thin data.
///
/// # Errors
/// [`ToolError`] when an instant leaves the Jiff range or a time zone is
/// missing.
pub fn print_evaluation(
    data: &BacktestData,
    flags: &EvalFlags,
    out: &mut Output,
) -> Result<bool, ToolError> {
    let evaluation = evaluate(data, &flags.options).map_err(|e| match e {
        crate::evaluation::EvalError::Backtest(b) => ToolError::Backtest(b),
        crate::evaluation::EvalError::HouseholdTimeZone(z) => ToolError::Argument(z.to_string()),
    })?;
    let mut thin = false;
    for &hour in &evaluation.issuance_hours_utc {
        let n = evaluation
            .issues
            .iter()
            .filter(|i| i.hour_utc == hour && matches!(i.status, IssueStatus::Forecast(_)))
            .count();
        if n < MIN_SCOREABLE_ORIGINS {
            thin = true;
            out.warn(&format!(
                "WARNING: only {n} issuances at {}:00 UTC have a forecast (need {MIN_SCOREABLE_ORIGINS}) — the evaluation is not yet a verdict.",
                hh(hour)
            ));
        }
    }
    if flags.bootstrap.block_days < MIN_OVERLAP_BLOCK_DAYS {
        out.warn(&format!(
            "WARNING: blocks of {} days are shorter than the {MIN_OVERLAP_BLOCK_DAYS}-day target span of one issuance — the intervals understate the uncertainty.",
            flags.bootstrap.block_days
        ));
    }
    let leaky = evaluation.issues.iter().any(|i| match &i.status {
        IssueStatus::Forecast(f) => !f.leak_free,
        _ => false,
    });
    if leaky {
        out.warn("WARNING: an issuance read an input after its issue time — the evaluation is not leak-free.");
    }
    print_header(data, flags, out);

    let segments: Vec<Segment> = if flags.split.is_some() {
        vec![Segment::Train, Segment::Selection, Segment::Test]
    } else {
        vec![Segment::All]
    };
    let final_segment = *segments.last().unwrap_or(&Segment::All);
    for segment in segments {
        if segment == Segment::Test && flags.criteria.is_none() {
            let n = evaluation
                .issues
                .iter()
                .filter(|i| segment.contains(i.t_ms, flags.split))
                .count();
            out.log(&format!(
                "\n== segment: test  {n} candidate issuances. SEALED: declare success and regression criteria with --criteria before the final test is shown."
            ));
            continue;
        }
        let stats = statistics(&evaluation, segment, flags.split, &flags.bootstrap);
        print_segment(&evaluation, flags, segment, &stats, out);
        if segment == final_segment
            && let Some(declared) = &flags.criteria
        {
            print_criteria(declared, segment, &stats, out);
        }
    }

    out.log("\n  CAVEATS (read before quoting any number):");
    out.log("   - Nearby issuances share targets and weather. Read the block-bootstrap interval, not the count of origins.");
    out.log("   - A window of a few months shows one season. It is not a year-round result.");
    out.log(&format!(
        "   - Results of the older `backtest` report (13:00 UTC only, horizons d+1…d+3 from the series start) are not {EVAL_VERSION} results."
    ));
    out.log("");
    Ok(!thin)
}
