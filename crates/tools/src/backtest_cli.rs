//! `spot-price-tools backtest`: replay a fixture (`--data`) or score the
//! database (`--db`). With `--db`, `--export` writes a fixture snapshot and
//! `--compare` measures the vintage-leak optimism. With either source, `--ab`
//! scores the production model (A) and a variant (B) on the same origins.
//! Exit codes: 0 for a verdict, 1 for thin data or an error, 2 for usage.

use std::collections::BTreeMap;
use std::path::Path;

use jiff::Timestamp;
use spot_price_domain::forecast::{
    DeriveOptions, FLOOR_HISTORY_DAYS, LatestPublishedRule, derive_band_offsets,
    observed_coverage_of,
};
use spot_price_domain::js::string_to_number;
use spot_price_domain::price::eur_mwh_to_cents_kwh;

use crate::backtest::{
    BacktestOptions, DayBasis, ForecastVariant, HORIZON_LABELS, HorizonPairedDelta,
    OptimismComparison, PRECISION_N_QUARTERS, PairedDelta, VariantComparison, compare_optimism,
    compare_variants, run_backtest,
};
use crate::cli::{Context, Output, flag_value, has_flag};
use crate::data::{BacktestData, DatasetId, GridActual, PricePoint, Vintage, load_fixture};
use crate::db;
use crate::error::ToolError;
use crate::jsfmt::to_fixed;
use crate::profile::ShapeFamily;

/// The default `--db` window in days back from now.
pub const DEFAULT_WINDOW_DAYS: i64 = 90;

/// Below this many scored origins the metrics are not a verdict.
pub const MIN_SCOREABLE_ORIGINS: usize = 14;

/// FI and the neighbour areas that the model lags on.
pub const SCORED_AREAS: [&str; 4] = ["FI", "SE1", "SE3", "EE"];

/// Above this sparse-ladder fraction the leak delta is likely understated.
const SPARSE_WARN: f64 = 0.2;

/// The largest extension window that `--ab` accepts. An extended quarter is
/// before the series end, so a lookback of more weeks reaches only actuals
/// older than [`FLOOR_HISTORY_DAYS`] before the series start. The served
/// window holds none, and such a variant would score like this cap.
#[expect(
    clippy::cast_possible_truncation,
    reason = "FLOOR_HISTORY_DAYS / 7 is a small positive number"
)]
pub const MAX_EXTENSION_WEEKS: u32 = (FLOOR_HISTORY_DAYS / 7) as u32;

const USAGE: &str = "usage: spot-price-tools backtest --data <fixture.json> | --db [--window <days>] [--export <fixture.json>] [--compare] [--db-url <url>] [--ab <key=value[,key=value]>]
  --ab  paired A/B: A = production model, B = production with these overrides, scored on the same origins.
        keys: ridge_lambda, wind_extension_weeks, consumption_extension_weeks,
        latest_published (before | at_or_before), prev_day_basis (utc | delivery),
        shape_profile (served | prev_day | prev_week | mean2w | mean3w | mean4w). Not with --compare.
  --eval  the evaluation report: --issuances <h[,h]> (UTC, default 6,13), --cost spot | contract:margin=,day=,night=,tax=,vat=,night_start=,night_end=,
          --split <selection-start>,<test-start> (YYYY-MM-DD), --criteria <file.json>, --seed <n>, --resamples <n>, --block-days <n>. With --ab, B is a second arm. Not with --compare.
          --profiles <family[,family]> adds per family a served-model arm shape_<family> and a pure-profile arm pure_<family>;
          families prev_day, prev_week, mean2w, mean3w, mean4w.";

/// One stored day-ahead price in EUR/MWh.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredPrice {
    pub area: String,
    pub delivery_start: Timestamp,
    pub price_eur_mwh: f64,
}

/// The backtest data from stored rows. Each price is converted from EUR/MWh
/// to c/kWh exactly once.
#[must_use]
pub fn assemble_backtest_data(
    prices_by_area: &[(String, Vec<StoredPrice>)],
    actuals: BTreeMap<DatasetId, Vec<GridActual>>,
    vintages: BTreeMap<DatasetId, Vec<Vintage>>,
) -> BacktestData {
    let prices = prices_by_area
        .iter()
        .flat_map(|(area, rows)| {
            rows.iter().map(move |row| PricePoint {
                area: area.clone(),
                start: row.delivery_start,
                spot_cents_kwh: eur_mwh_to_cents_kwh(row.price_eur_mwh),
            })
        })
        .collect();
    BacktestData {
        prices,
        actuals,
        vintages,
    }
}

/// Whether too few origins were scored for a verdict.
#[must_use]
pub const fn is_thin_data(origins: usize) -> bool {
    origins < MIN_SCOREABLE_ORIGINS
}

/// `--window <days>`: a positive whole number like `Number(raw)` reads it;
/// [`DEFAULT_WINDOW_DAYS`] when absent.
///
/// # Errors
/// [`ToolError::Argument`] for any other value.
pub fn parse_window_days(raw: Option<&str>) -> Result<i64, ToolError> {
    let Some(raw) = raw else {
        return Ok(DEFAULT_WINDOW_DAYS);
    };
    let value = string_to_number(raw);
    let invalid = || {
        ToolError::Argument(format!(
            "--window must be a positive integer number of days, got \"{raw}\""
        ))
    };
    if !value.is_finite() || value.fract() != 0.0 || value <= 0.0 {
        return Err(invalid());
    }
    // A whole number of at most 1e9 days is exact as i64.
    if value > 1e9 {
        return Err(ToolError::Argument(format!(
            "--window is out of range, got \"{raw}\""
        )));
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "value is a whole number in 1..=1e9"
    )]
    Ok(value as i64)
}

/// `--ab <key=value[,key=value]>`: the production options with overrides.
/// `ridge_lambda` is a finite number ≥ 0. The `*_extension_weeks` keys are
/// whole numbers in `0..=MAX_EXTENSION_WEEKS`. `latest_published` is
/// `before` or `at_or_before`. `prev_day_basis` is `utc` or `delivery`.
/// `shape_profile` is a [`ShapeFamily::token`]. A key may appear once.
///
/// # Errors
/// [`ToolError::Argument`] for an unknown or repeated key, or a bad value.
pub fn parse_variant(raw: &str) -> Result<ForecastVariant, ToolError> {
    let mut variant = ForecastVariant::default();
    let mut seen: Vec<&str> = Vec::new();
    for item in raw.split(',') {
        let Some((key, value)) = item.split_once('=') else {
            return Err(ToolError::Argument(format!(
                "--ab expects key=value pairs separated by commas, got \"{raw}\""
            )));
        };
        let (key, value) = (key.trim(), value.trim());
        if seen.contains(&key) {
            return Err(ToolError::Argument(format!(
                "--ab sets {key} more than once"
            )));
        }
        match key {
            "ridge_lambda" => variant.ridge_lambda = parse_ridge_lambda(value)?,
            "wind_extension_weeks" => variant.wind_extension_weeks = parse_weeks(key, value)?,
            "consumption_extension_weeks" => {
                variant.consumption_extension_weeks = parse_weeks(key, value)?;
            }
            "latest_published" => variant.latest_published = parse_latest_published(value)?,
            "prev_day_basis" => variant.prev_day_basis = parse_prev_day_basis(value)?,
            "shape_profile" => {
                variant.shape_profile = ShapeFamily::parse(value)
                    .map_err(|message| ToolError::Argument(format!("--ab {message}")))?;
            }
            _ => {
                return Err(ToolError::Argument(format!(
                    "--ab key must be ridge_lambda, wind_extension_weeks, consumption_extension_weeks, latest_published, prev_day_basis, or shape_profile, got \"{key}\""
                )));
            }
        }
        seen.push(key);
    }
    Ok(variant)
}

fn parse_ridge_lambda(value: &str) -> Result<f64, ToolError> {
    let lambda = string_to_number(value);
    if value.is_empty() || !lambda.is_finite() || lambda < 0.0 {
        return Err(ToolError::Argument(format!(
            "--ab ridge_lambda must be a finite number >= 0, got \"{value}\""
        )));
    }
    Ok(lambda)
}

fn parse_weeks(key: &str, value: &str) -> Result<u32, ToolError> {
    let weeks = string_to_number(value);
    if value.is_empty()
        || !weeks.is_finite()
        || weeks.fract() != 0.0
        || weeks < 0.0
        || weeks > f64::from(MAX_EXTENSION_WEEKS)
    {
        return Err(ToolError::Argument(format!(
            "--ab {key} must be a whole number from 0 to {MAX_EXTENSION_WEEKS}, got \"{value}\""
        )));
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "weeks is a whole number in 0..=MAX_EXTENSION_WEEKS"
    )]
    Ok(weeks as u32)
}

fn parse_latest_published(value: &str) -> Result<LatestPublishedRule, ToolError> {
    match value {
        "at_or_before" => Ok(LatestPublishedRule::AtOrBefore),
        "before" => Ok(LatestPublishedRule::Before),
        _ => Err(ToolError::Argument(format!(
            "--ab latest_published must be before or at_or_before, got \"{value}\""
        ))),
    }
}

/// The `--ab` value of a [`LatestPublishedRule`].
const fn latest_published_name(rule: LatestPublishedRule) -> &'static str {
    match rule {
        LatestPublishedRule::AtOrBefore => "at_or_before",
        LatestPublishedRule::Before => "before",
    }
}

fn parse_prev_day_basis(value: &str) -> Result<DayBasis, ToolError> {
    match value {
        "utc" => Ok(DayBasis::Utc),
        "delivery" => Ok(DayBasis::Delivery),
        _ => Err(ToolError::Argument(format!(
            "--ab prev_day_basis must be utc or delivery, got \"{value}\""
        ))),
    }
}

/// The `--ab` value of a [`DayBasis`].
const fn prev_day_basis_name(basis: DayBasis) -> &'static str {
    match basis {
        DayBasis::Utc => "utc",
        DayBasis::Delivery => "delivery",
    }
}

/// `v.toFixed(digits)`, or `n/a`.
pub(crate) fn fmt(v: Option<f64>, digits: usize) -> String {
    v.map_or_else(|| "n/a".to_owned(), |v| to_fixed(v, digits))
}

fn sub(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    Some(a? - b?)
}

/// Prints the backtest report; returns false for thin data.
///
/// # Errors
/// [`ToolError`] when a forecast window leaves the `Timestamp` range.
pub fn print_report(data: &BacktestData, out: &mut Output) -> Result<bool, ToolError> {
    let summary = run_backtest(data, &BacktestOptions::default())?;
    let derived = derive_band_offsets(&summary.residuals, &DeriveOptions::default());
    let coverage = observed_coverage_of(
        &summary.residuals,
        &derived.offsets_by_hour,
        derived.global_offsets,
    );
    let hour_buckets = derived.offsets_by_hour.iter().flatten().count();

    let thin = is_thin_data(summary.origins);
    if thin {
        out.warn(&format!(
            "WARNING: only {} scoreable origins (need {MIN_SCOREABLE_ORIGINS}) — metrics are not yet a verdict.",
            summary.origins
        ));
    }

    out.log("\nFI forecast backtest (issue-time keyed)");
    out.log(&format!("  origins scored:      {}", summary.origins));
    out.log(&format!(
        "  fallback origins:    {}",
        summary.fallback_origins
    ));
    out.log(&format!(
        "  sanity-clamped quarters: {}",
        summary.sanity_clamped_quarters
    ));
    out.log(&format!(
        "  no-grid-data origins: {}",
        summary.no_grid_data_origins
    ));
    out.log(&format!("  leakage-free:        {}", summary.leak_free));
    out.log(&format!(
        "  model MAE  (c/kWh):  {}",
        fmt(summary.model_mae, 3)
    ));
    out.log(&format!(
        "  model sMAPE:         {}",
        fmt(summary.model_smape, 3)
    ));
    out.log(&format!(
        "  rMAE vs last_week:           {}",
        fmt(summary.r_mae.last_week, 3)
    ));
    out.log(&format!(
        "  rMAE vs last_published_day:  {}",
        fmt(summary.r_mae.last_published_day, 3)
    ));
    out.log(&format!(
        "  band residuals:      {}  hour-buckets {hour_buckets}  observed coverage {}",
        summary.residuals.len(),
        to_fixed(coverage, 3)
    ));

    out.log(&format!(
        "\n  per horizon (precision@N over the cheapest/peak {PRECISION_N_QUARTERS} quarters = {}h):",
        PRECISION_N_QUARTERS / 4
    ));
    out.log("    horizon  pairs     MAE   sMAPE   Spearman  P@N-cheap  P@N-peak  bandCov");
    for (label, m) in HORIZON_LABELS.iter().zip(&summary.by_horizon) {
        out.log(&format!(
            "    {label:<7}  {:>5}  {:>6}  {:>6}  {:>8}  {:>9}  {:>8}  {:>7}",
            m.pairs,
            fmt(m.mae, 3),
            fmt(m.smape, 3),
            fmt(m.spearman, 3),
            fmt(m.precision_at_n_cheap, 3),
            fmt(m.precision_at_n_peak, 3),
            fmt(m.band_coverage, 3),
        ));
    }
    out.log("");
    Ok(!thin)
}

/// Prints the vintage-leak optimism comparison; returns false for thin data.
///
/// # Errors
/// [`ToolError`] when a forecast window leaves the `Timestamp` range.
pub fn print_compare(data: &BacktestData, out: &mut Output) -> Result<bool, ToolError> {
    let cmp = compare_optimism(data)?;
    let thin = is_thin_data(cmp.scored_origins);
    if thin {
        out.warn(&format!(
            "WARNING: only {} shared origins (need {MIN_SCOREABLE_ORIGINS}) — the delta is not yet a verdict.",
            cmp.scored_origins
        ));
    }
    print_compare_header(&cmp, out);
    print_compare_table(&cmp, out);
    Ok(!thin)
}

fn print_compare_header(cmp: &OptimismComparison, out: &mut Output) {
    let honest = &cmp.honest;
    out.log("\nVintage-leak optimism (leaked latest vs honest issue-time)");
    out.log(&format!(
        "  shared origins:      {}  (pre-vintage skipped: {})",
        cmp.scored_origins, honest.pre_vintage_origins
    ));
    out.log(&format!(
        "  scored window:       {} … {}  ({} days)",
        iso_or_na(honest.scored_window_start_ms),
        iso_or_na(honest.scored_window_end_ms),
        to_fixed(honest.scored_window_span_days, 1)
    ));
    out.log(&format!(
        "  honest leak-free:    {}   leaked leak-free: {}  (leaked is FALSE BY DESIGN — the vintage postdates issue time)",
        honest.leak_free, cmp.leaked.leak_free
    ));
    out.log(&format!(
        "  sanity-clamped quarters: honest {}  leaked {}",
        honest.sanity_clamped_quarters, cmp.leaked.sanity_clamped_quarters
    ));
    out.log(&format!(
        "  no-grid-data origins: honest {}  leaked {}",
        honest.no_grid_data_origins, cmp.leaked.no_grid_data_origins
    ));

    out.log("\n  CAVEATS (read before quoting any number):");
    out.log("   - SIGN: delta = leaked − honest (positive ⇒ leaked was optimistic).");
    out.log(&format!(
        "   - N={} DAILY origins, strongly autocorrelated → treat as DIRECTIONAL, not a precise interval (no CI is reported).",
        cmp.scored_origins
    ));
    out.log("   - SUMMER WIND-ONLY LOWER BOUND on the annual leak: 165 shows no summer revision (#79), so winter would only widen it.");
    out.log(
        "   - delta ≈ 0 ALONE MUST NOT close #81 — re-run with autumn/winter accumulation first.",
    );
}

fn print_compare_table(cmp: &OptimismComparison, out: &mut Output) {
    let (leaked, honest) = (&cmp.leaked, &cmp.honest);
    out.log("\n  metric            leaked      honest       delta");
    let mut row = |label: &str, l: Option<f64>, h: Option<f64>| {
        out.log(&format!(
            "  {label:<16}{:>8}    {:>8}    {:>8}",
            fmt(l, 3),
            fmt(h, 3),
            fmt(sub(l, h), 3)
        ));
    };
    row("MAE (c/kWh)", leaked.model_mae, honest.model_mae);
    row("sMAPE", leaked.model_smape, honest.model_smape);
    row(
        "rMAE last_week",
        leaked.r_mae.last_week,
        honest.r_mae.last_week,
    );
    row(
        "rMAE last_pub_day",
        leaked.r_mae.last_published_day,
        honest.r_mae.last_published_day,
    );
    out.log(&format!(
        "  band coverage     {:>8}    {:>8}    {:>8}",
        "—",
        "—",
        fmt(cmp.delta_band_coverage, 3)
    ));

    out.log("\n  per horizon (delta = leaked − honest):");
    out.log("    horizon   deltaMAE   deltaSMAPE   deltaBandCov");
    for (label, d) in HORIZON_LABELS.iter().zip(&cmp.delta_by_horizon) {
        out.log(&format!(
            "    {label:<7}   {:>8}   {:>10}   {:>12}",
            fmt(d.delta_mae, 3),
            fmt(d.delta_smape, 3),
            fmt(d.delta_band_coverage, 3)
        ));
    }

    out.log("\n  ladder depth (cron-gap diagnostic):");
    out.log("    dataset   medianDepth   expectedDepth(p90)   sparseFrac");
    for (id, ld) in &cmp.ladder_diagnostic {
        out.log(&format!(
            "    {:<7}   {:>11}   {:>18}   {:>10}",
            id.to_string(),
            fmt(ld.median_depth, 1),
            fmt(ld.expected_depth, 1),
            fmt(ld.sparse_target_fraction, 3)
        ));
        if ld.sparse_target_fraction.is_some_and(|f| f > SPARSE_WARN) {
            out.warn(&format!(
                "    WARNING: dataset {id} has {} thin ladders — cron gaps may UNDERSTATE the delta.",
                fmt(ld.sparse_target_fraction, 3)
            ));
        }
    }
    out.log("");
}

/// Prints the paired A/B comparison of the production model (A) and
/// `variant` (B); returns false for thin data or unpaired origins.
///
/// # Errors
/// [`ToolError`] when a forecast window leaves the `Timestamp` range.
pub fn print_variant_compare(
    data: &BacktestData,
    variant: ForecastVariant,
    out: &mut Output,
) -> Result<bool, ToolError> {
    let cmp = compare_variants(data, ForecastVariant::default(), variant)?;
    let thin = is_thin_data(cmp.shared_origins);
    if thin {
        out.warn(&format!(
            "WARNING: only {} shared origins (need {MIN_SCOREABLE_ORIGINS}) — the delta is not yet a verdict.",
            cmp.shared_origins
        ));
    }
    if !cmp.origins_match {
        out.warn("WARNING: arm B did not score the same origins and targets as arm A — the deltas are not a verdict.");
    }
    if cmp.a.fallback_origins != cmp.b.fallback_origins {
        out.warn(&format!(
            "WARNING: fallback origins differ (A {}, B {}) — the rank means cover different origins.",
            cmp.a.fallback_origins, cmp.b.fallback_origins
        ));
    }
    print_variant_header(&cmp, out);
    print_variant_table(&cmp, out);
    Ok(!thin && cmp.origins_match)
}

pub(crate) fn variant_label(v: ForecastVariant) -> String {
    let label = format!(
        "ridge_lambda={} wind_extension_weeks={} consumption_extension_weeks={} latest_published={} prev_day_basis={}",
        v.ridge_lambda,
        v.wind_extension_weeks,
        v.consumption_extension_weeks,
        latest_published_name(v.latest_published),
        prev_day_basis_name(v.prev_day_basis)
    );
    // The served profile is not named, so the labels of older runs stay.
    if v.shape_profile == ShapeFamily::Served {
        label
    } else {
        format!("{label} shape_profile={}", v.shape_profile.token())
    }
}

fn print_variant_header(cmp: &VariantComparison, out: &mut Output) {
    let (a, b) = (&cmp.a, &cmp.b);
    out.log("\nPaired A/B backtest (issue-time keyed; A = production, B = variant)");
    out.log(&format!(
        "  arm A:               {}",
        variant_label(cmp.variant_a)
    ));
    out.log(&format!(
        "  arm B:               {}",
        variant_label(cmp.variant_b)
    ));
    out.log(&format!(
        "  shared origins:      {}  (pre-vintage skipped: {})",
        cmp.shared_origins, a.pre_vintage_origins
    ));
    out.log(&format!(
        "  scored window:       {} … {}  ({} days)",
        iso_or_na(a.scored_window_start_ms),
        iso_or_na(a.scored_window_end_ms),
        to_fixed(a.scored_window_span_days, 1)
    ));
    out.log(&format!("  origins paired:      {}", cmp.origins_match));
    out.log(&format!(
        "  leak-free:           A {}  B {}",
        a.leak_free, b.leak_free
    ));
    out.log(&format!(
        "  fallback origins:    A {}  B {}",
        a.fallback_origins, b.fallback_origins
    ));
    out.log(&format!(
        "  sanity-clamped quarters: A {}  B {}",
        a.sanity_clamped_quarters, b.sanity_clamped_quarters
    ));
    out.log(&format!(
        "  no-grid-data origins: A {}  B {}",
        a.no_grid_data_origins, b.no_grid_data_origins
    ));

    out.log("\n  CAVEATS (read before quoting any number):");
    out.log("   - SIGN: delta = B − A. Rank metrics: positive ⇒ B ranks better. MAE: negative ⇒ B is more accurate.");
    out.log(&format!(
        "   - N={} DAILY origins, strongly autocorrelated → treat as DIRECTIONAL, not a precise interval (no CI is reported).",
        cmp.shared_origins
    ));
    out.log("   - This report gives deltas, not a verdict. Fix the per-horizon threshold in the issue BEFORE the run.");
    out.log("   - Judge that threshold on the PAIRED MAE table: the same origins in both arms. The tool applies no threshold.");
}

fn print_variant_table(cmp: &VariantComparison, out: &mut Output) {
    let (a, b) = (&cmp.a, &cmp.b);
    out.log(&format!(
        "\n  {:<20}  {:>8}  {:>8}  {:>8}",
        "overall", "A", "B", "delta"
    ));
    let mut overall = |name: &str, va: Option<f64>, vb: Option<f64>| {
        out.log(&format!(
            "  {name:<20}  {:>8}  {:>8}  {:>8}",
            fmt(va, 3),
            fmt(vb, 3),
            fmt(sub(vb, va), 3)
        ));
    };
    overall("MAE (c/kWh)", a.model_mae, b.model_mae);
    overall("sMAPE", a.model_smape, b.model_smape);
    overall("rMAE last_week", a.r_mae.last_week, b.r_mae.last_week);
    overall(
        "rMAE last_pub_day",
        a.r_mae.last_published_day,
        b.r_mae.last_published_day,
    );

    out.log(
        "\n  REFERENCE ONLY: the next table compares arm means, which can cover different origins.",
    );
    out.log(&format!(
        "  per horizon (precision@N over the cheapest/peak {PRECISION_N_QUARTERS} quarters = {}h; delta = B − A):",
        PRECISION_N_QUARTERS / 4
    ));
    out.log(&format!(
        "    {:<7}  {:>5}  {:<16}  {:>8}  {:>8}  {:>8}",
        "horizon", "pairs", "metric", "A", "B", "delta"
    ));
    let rows = HORIZON_LABELS
        .iter()
        .zip(cmp.a.by_horizon.iter().zip(&cmp.b.by_horizon))
        .zip(&cmp.delta_by_horizon);
    for ((label, (a, b)), d) in rows {
        let metrics = [
            ("Spearman", a.spearman, b.spearman, d.delta_spearman),
            (
                "P@N-cheap",
                a.precision_at_n_cheap,
                b.precision_at_n_cheap,
                d.delta_precision_at_n_cheap,
            ),
            (
                "P@N-peak",
                a.precision_at_n_peak,
                b.precision_at_n_peak,
                d.delta_precision_at_n_peak,
            ),
            ("MAE (c/kWh, ref)", a.mae, b.mae, d.delta_mae),
        ];
        for (i, (name, va, vb, delta)) in metrics.into_iter().enumerate() {
            let (horizon, pairs) = if i == 0 {
                ((*label).to_owned(), a.pairs.to_string())
            } else {
                (String::new(), String::new())
            };
            out.log(&format!(
                "    {horizon:<7}  {pairs:>5}  {name:<16}  {:>8}  {:>8}  {:>8}",
                fmt(va, 3),
                fmt(vb, 3),
                fmt(delta, 3)
            ));
        }
    }
    print_paired_table(&cmp.paired_by_horizon, out);
    out.log("");
}

/// Prints the paired deltas: the MAE table, which is the verdict, then the
/// rank metrics as information. One row per horizon and metric.
pub fn print_paired_table(paired: &[HorizonPairedDelta; 3], out: &mut Output) {
    out.log("\n  PAIRED MAE per horizon (the A/B verdict lines; delta = B − A in c/kWh per origin where both arms have a value):");
    out.log("    W = B more accurate (delta < 0), L = B less accurate, T = exactly equal; excluded = only one arm has a value, or the delta is not finite.");
    print_paired_rows(paired, &[("MAE", |p| p.mae)], out);

    out.log("\n  PAIRED rank metrics per horizon (information, not the verdict; delta = B − A per origin where both arms have a value):");
    out.log("    Each horizon day starts at the CET/CEST delivery-day boundary, so it spans two UTC days and its ranks can react to model options.");
    out.log("    W = B ranks better (delta > 0), L = B ranks worse, T = exactly equal; excluded = only one arm has a value, or the delta is not finite.");
    print_paired_rows(
        paired,
        &[
            ("Spearman", |p| p.spearman),
            ("P@N-cheap", |p| p.precision_at_n_cheap),
            ("P@N-peak", |p| p.precision_at_n_peak),
        ],
        out,
    );
}

type PairedMetric = fn(&HorizonPairedDelta) -> PairedDelta;

fn print_paired_rows(
    paired: &[HorizonPairedDelta; 3],
    metrics: &[(&str, PairedMetric)],
    out: &mut Output,
) {
    out.log(&format!(
        "    {:<7}  {:<9}  {:>6}  {:>9}  {:>5}  {:>5}  {:>5}  {:>8}",
        "horizon", "metric", "paired", "meanDelta", "W", "L", "T", "excluded"
    ));
    for (label, p) in HORIZON_LABELS.iter().zip(paired) {
        for (i, (name, metric)) in metrics.iter().enumerate() {
            let d = metric(p);
            let horizon = if i == 0 { *label } else { "" };
            out.log(&format!(
                "    {horizon:<7}  {name:<9}  {:>6}  {:>9}  {:>5}  {:>5}  {:>5}  {:>8}",
                d.paired,
                fmt(d.mean_delta, 3),
                d.wins,
                d.losses,
                d.ties,
                d.excluded
            ));
        }
    }
}

/// `Date.prototype.toISOString` of a UTC ms instant, or `n/a`.
pub(crate) fn iso_or_na(ms: Option<i64>) -> String {
    ms.and_then(|ms| Timestamp::from_millisecond(ms).ok())
        .map_or_else(|| "n/a".to_owned(), spot_price_domain::forecast::iso_millis)
}

/// The database URL: `--db-url`, else `DATABASE_PUBLIC_URL`.
pub(crate) fn database_url(
    args: &[String],
    ctx: &Context<'_>,
    command: &'static str,
) -> Result<String, ToolError> {
    flag_value(args, "--db-url")
        .map(str::to_owned)
        .or_else(|| (ctx.env)("DATABASE_PUBLIC_URL"))
        .filter(|url| !url.is_empty())
        .ok_or(ToolError::MissingDatabaseUrl(command))
}

/// Reads the `--db` window and writes the `--export` snapshot when asked.
fn read_db(
    args: &[String],
    ctx: &Context<'_>,
    out: &mut Output,
) -> Result<BacktestData, ToolError> {
    let window_days = parse_window_days(flag_value(args, "--window"))?;
    let url = database_url(args, ctx, "spot-price-tools backtest --db")?;
    let data = db::block_on(db::fetch_backtest_data(&url, window_days, ctx.now))??;

    if let Some(file) = flag_value(args, "--export") {
        std::fs::write(file, crate::data::to_fixture_json(&data)).map_err(|source| {
            ToolError::Write {
                path: file.to_owned(),
                source,
            }
        })?;
        out.log(&format!("Exported {} prices → {file}", data.prices.len()));
    }
    Ok(data)
}

/// `backtest --eval`: every flag is checked before any data is read.
fn run_eval(args: &[String], ctx: &Context<'_>, out: &mut Output) -> Result<i32, ToolError> {
    if has_flag(args, "--compare") {
        return Err(ToolError::Argument(
            "--eval and --compare cannot be used together".to_owned(),
        ));
    }
    let flags = crate::eval_cli::parse_flags(args)?;
    let data = if let Some(file) = flag_value(args, "--data") {
        load_fixture(Path::new(file))?
    } else if has_flag(args, "--db") {
        read_db(args, ctx, out)?
    } else {
        out.error(USAGE);
        return Ok(2);
    };
    let ok = crate::eval_cli::print_evaluation(&data, &flags, out)?;
    Ok(i32::from(!ok))
}

/// Runs `backtest` with the arguments after the subcommand.
///
/// # Errors
/// [`ToolError`] for an unreadable fixture, a bad flag, or a database error.
pub fn run(args: &[String], ctx: &Context<'_>, out: &mut Output) -> Result<i32, ToolError> {
    if has_flag(args, "--eval") {
        return run_eval(args, ctx, out);
    }
    let variant = if has_flag(args, "--ab") {
        let raw = flag_value(args, "--ab")
            .ok_or_else(|| ToolError::Argument("--ab needs a key=value list".to_owned()))?;
        if has_flag(args, "--compare") {
            return Err(ToolError::Argument(
                "--ab and --compare cannot be used together".to_owned(),
            ));
        }
        Some(parse_variant(raw)?)
    } else {
        None
    };

    // `--data` wins over every other flag except `--ab`, like the TypeScript CLI.
    if let Some(file) = flag_value(args, "--data") {
        let data = load_fixture(Path::new(file))?;
        let ok = match variant {
            Some(variant) => print_variant_compare(&data, variant, out)?,
            None => print_report(&data, out)?,
        };
        return Ok(i32::from(!ok));
    }
    if !has_flag(args, "--db") {
        out.error(USAGE);
        return Ok(2);
    }

    let data = read_db(args, ctx, out)?;

    let ok = if let Some(variant) = variant {
        print_variant_compare(&data, variant, out)?
    } else if has_flag(args, "--compare") {
        print_compare(&data, out)?
    } else {
        print_report(&data, out)?
    };
    Ok(i32::from(!ok))
}
