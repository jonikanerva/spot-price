//! `spot-price-tools backtest`: replay a fixture (`--data`) or score the
//! database (`--db`). With `--db`, `--export` writes a fixture snapshot and
//! `--compare` measures the vintage-leak optimism. Exit codes: 0 for a
//! verdict, 1 for thin data or an error, 2 for usage.

use std::collections::BTreeMap;
use std::path::Path;

use jiff::Timestamp;
use spot_price_domain::forecast::{DeriveOptions, derive_band_offsets, observed_coverage_of};
use spot_price_domain::js::string_to_number;
use spot_price_domain::price::eur_mwh_to_cents_kwh;

use crate::backtest::{
    BacktestOptions, HORIZON_LABELS, OptimismComparison, PRECISION_N_QUARTERS, compare_optimism,
    run_backtest,
};
use crate::cli::{Context, Output, flag_value, has_flag};
use crate::data::{BacktestData, DatasetId, GridActual, PricePoint, Vintage, load_fixture};
use crate::db;
use crate::error::ToolError;
use crate::jsfmt::to_fixed;

/// The default `--db` window in days back from now.
pub const DEFAULT_WINDOW_DAYS: i64 = 90;

/// Below this many scored origins the metrics are not a verdict.
pub const MIN_SCOREABLE_ORIGINS: usize = 14;

/// FI and the neighbour areas that the model lags on.
pub const SCORED_AREAS: [&str; 4] = ["FI", "SE1", "SE3", "EE"];

/// Above this sparse-ladder fraction the leak delta is likely understated.
const SPARSE_WARN: f64 = 0.2;

const USAGE: &str = "usage: spot-price-tools backtest --data <fixture.json> | --db [--window <days>] [--export <fixture.json>] [--compare] [--db-url <url>]";

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

/// Runs `backtest` with the arguments after the subcommand.
///
/// # Errors
/// [`ToolError`] for an unreadable fixture, a bad flag, or a database error.
pub fn run(args: &[String], ctx: &Context<'_>, out: &mut Output) -> Result<i32, ToolError> {
    // `--data` wins over every other flag, like the TypeScript CLI.
    if let Some(file) = flag_value(args, "--data") {
        let data = load_fixture(Path::new(file))?;
        let ok = print_report(&data, out)?;
        return Ok(i32::from(!ok));
    }
    if !has_flag(args, "--db") {
        out.error(USAGE);
        return Ok(2);
    }

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

    let ok = if has_flag(args, "--compare") {
        print_compare(&data, out)?
    } else {
        print_report(&data, out)?
    };
    Ok(i32::from(!ok))
}
