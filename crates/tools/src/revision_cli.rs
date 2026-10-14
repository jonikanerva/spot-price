//! `spot-price-tools revision-magnitude`: the forecast-revision study over a
//! vintage fixture (`--data`) or the live database (`--db`), with a
//! GO / MARGINAL / DEFER recommendation.
//!
//! Exit codes: 0 for GO or MARGINAL, 1 for DEFER or an error, 2 for usage.

use std::collections::BTreeMap;
use std::path::Path;

use jiff::Timestamp;
use serde::Deserialize;
use spot_price_domain::forecast::iso_millis;

use crate::backtest_cli::{database_url, fmt, parse_window_days};
use crate::cli::{Context, Output, flag_value, has_flag};
use crate::data::{DatasetId, Vintage, parse_dataset_key, parse_instant, read_json};
use crate::db;
use crate::error::ToolError;
use crate::jsfmt::JsJson;
use crate::revision::{
    ActualRecord, DatasetRevisionSummary, REFERENCE_MAX_LEAD_H, RevisionStudyInput, Verdict,
    recommendation, run_revision_study,
};

/// Above this exclusion rate the report warns that the reference is weak.
const EXCLUSION_WARN: f64 = 0.25;

const USAGE: &str = "usage: spot-price-tools revision-magnitude --data <fixture.json> | --db [--window <days>] [--export <fixture.json>] [--db-url <url>]";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawVintage {
    dataset_id: DatasetId,
    issued_at: String,
    start_time: String,
    end_time: String,
    value: f64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawActual {
    dataset_id: DatasetId,
    start_time: String,
    value: f64,
}

#[derive(Deserialize)]
struct RawVintageFixture {
    vintages: BTreeMap<String, Vec<RawVintage>>,
    actuals: Option<BTreeMap<String, Vec<RawActual>>>,
}

fn from_raw(raw: RawVintageFixture) -> Result<RevisionStudyInput, String> {
    let mut input = RevisionStudyInput::default();
    for (key, records) in raw.vintages {
        let rows = records
            .iter()
            .map(|r| {
                Ok(Vintage {
                    dataset_id: r.dataset_id,
                    issued_at: parse_instant(&r.issued_at)?,
                    start: parse_instant(&r.start_time)?,
                    end: parse_instant(&r.end_time)?,
                    value: r.value,
                })
            })
            .collect::<Result<_, String>>()?;
        input
            .vintages_by_dataset
            .insert(parse_dataset_key(&key)?, rows);
    }
    for (key, records) in raw.actuals.unwrap_or_default() {
        let rows = records
            .iter()
            .map(|r| {
                Ok(ActualRecord {
                    dataset_id: r.dataset_id,
                    start: parse_instant(&r.start_time)?,
                    value: r.value,
                })
            })
            .collect::<Result<_, String>>()?;
        input
            .actuals_by_dataset
            .insert(parse_dataset_key(&key)?, rows);
    }
    Ok(input)
}

/// Reads a vintage fixture, the format that `--export` writes.
///
/// # Errors
/// [`ToolError`] when the file cannot be read, is not valid JSON, or has an
/// invalid timestamp or dataset key.
pub fn load_vintage_fixture(path: &Path) -> Result<RevisionStudyInput, ToolError> {
    let raw: RawVintageFixture = read_json(path)?;
    from_raw(raw).map_err(|reason| ToolError::Fixture {
        path: path.display().to_string(),
        reason,
    })
}

fn text(at: Timestamp) -> JsJson {
    JsJson::Text(iso_millis(at))
}

fn member(key: &str, value: JsJson) -> (String, JsJson) {
    (key.to_owned(), value)
}

/// The input as vintage fixture JSON that [`load_vintage_fixture`] reads
/// back unchanged.
#[must_use]
pub fn to_fixture_json(input: &RevisionStudyInput) -> String {
    let vintages = input
        .vintages_by_dataset
        .iter()
        .map(|(id, rows)| {
            let rows = rows
                .iter()
                .map(|r| {
                    JsJson::Object(vec![
                        member("datasetId", JsJson::Number(f64::from(r.dataset_id))),
                        member("issuedAt", text(r.issued_at)),
                        member("startTime", text(r.start)),
                        member("endTime", text(r.end)),
                        member("value", JsJson::Number(r.value)),
                    ])
                })
                .collect();
            (id.to_string(), JsJson::Array(rows))
        })
        .collect();
    let actuals = input
        .actuals_by_dataset
        .iter()
        .map(|(id, rows)| {
            let rows = rows
                .iter()
                .map(|r| {
                    JsJson::Object(vec![
                        member("datasetId", JsJson::Number(f64::from(r.dataset_id))),
                        member("startTime", text(r.start)),
                        member("value", JsJson::Number(r.value)),
                    ])
                })
                .collect();
            (id.to_string(), JsJson::Array(rows))
        })
        .collect();
    JsJson::Object(vec![
        member("vintages", JsJson::Object(vintages)),
        member("actuals", JsJson::Object(actuals)),
    ])
    .stringify_pretty()
}

fn day(at: Timestamp) -> String {
    iso_millis(at).chars().take(10).collect()
}

fn print_dataset(d: &DatasetRevisionSummary, out: &mut Output) {
    out.log(&format!("\ndataset {} (forecast):", d.dataset_id));
    out.log(&format!(
        "  targets:   admissible {}  excluded {}  future {}  exclusion-rate {}",
        d.admissible_targets,
        d.excluded_targets,
        d.future_targets,
        fmt(d.exclusion_rate, 3)
    ));
    if d.exclusion_rate.is_some_and(|rate| rate > EXCLUSION_WARN) {
        out.warn(&format!(
            "  WARNING: exclusion rate {} > {} — many targets lack a near-delivery reference (job gaps?); treat the reference as less reliable.",
            fmt(d.exclusion_rate, 3),
            fmt(Some(EXCLUSION_WARN), 2)
        ));
    }
    out.log(&format!(
        "  reference: post-delivery {}  pre-delivery≤{REFERENCE_MAX_LEAD_H}h {}",
        d.references_post_delivery, d.references_pre_delivery_within_tol
    ));
    out.log(&format!(
        "  empirical lead: max {}h  p90 {}h   (gate on this band — the served forecast has NO Fingrid feature beyond it)",
        fmt(d.empirical_max_lead_h, 1),
        fmt(d.empirical_p90_lead_h, 1)
    ));
    out.log(&format!(
        "  reference series: sd {} MW  mean|ref| {} MW",
        fmt(d.sd_reference, 1),
        fmt(d.mean_abs_reference, 1)
    ));
    match &d.actual_check {
        Some(check) => out.log(&format!(
            "  ref-vs-actual sanity: {} targets  median|ref−actual| {} MW  mean {} MW",
            check.targets_compared,
            fmt(check.median_abs_ref_minus_actual, 1),
            fmt(check.mean_abs_ref_minus_actual, 1)
        )),
        None => out.log("  ref-vs-actual sanity: no overlapping actuals (skipped)"),
    }
    out.log(&format!(
        "  aggregate (sufficient bins): rms {} MW  NSR {}  attenuation(illustrative) {}  samples {}",
        fmt(d.production_band_rms, 1),
        fmt(d.production_band_nsr, 3),
        fmt(d.attenuation_illustration, 3),
        d.production_band_samples
    ));
    out.log("    bin       samples  targets  meanAbsΔ  medAbsΔ  p90AbsΔ    rmsΔ   biasΔ    NSR   atten  relMeanAbs");
    for b in &d.buckets {
        if !b.sufficient {
            let tag = if b.samples > 0 {
                "insufficient"
            } else {
                "empty"
            };
            out.log(&format!(
                "    {:<8}  {:>7}  {:>7}  {tag}",
                b.label, b.samples, b.targets
            ));
            continue;
        }
        out.log(&format!(
            "    {:<8}  {:>7}  {:>7}  {:>8}  {:>7}  {:>7}  {:>6}  {:>6}  {:>5}  {:>6}  {:>8}",
            b.label,
            b.samples,
            b.targets,
            fmt(b.mean_abs_revision, 1),
            fmt(b.median_abs_revision, 1),
            fmt(b.p90_abs_revision, 1),
            fmt(b.rms_revision, 1),
            fmt(b.signed_bias_revision, 1),
            fmt(b.noise_to_signal, 3),
            fmt(b.attenuation, 3),
            fmt(b.rel_mean_abs, 3)
        ));
    }
}

/// Prints the study report; returns false for DEFER.
pub fn print_report(input: &RevisionStudyInput, out: &mut Output) -> bool {
    let result = run_revision_study(input);
    let rec = recommendation(&result);

    out.log("\nFingrid forecast-revision study (issue #79)");
    if let Some(w) = &result.window {
        out.log(&format!(
            "  window: targets {} … {}  (issued {} … {})",
            day(w.earliest_target),
            day(w.latest_target),
            day(w.earliest_issued_at),
            day(w.latest_issued_at)
        ));
        out.log("  CAVEAT (summer-only): this window covers a single-season regime; wind/consumption revision dynamics differ in winter, so a decision to CLOSE #81 cannot be recorded from this window alone — only GO / MARGINAL / DEFER (da amendment 1).");
    } else {
        out.log("  window: no vintages found");
    }
    out.log(&format!(
        "  CAVEAT (lead resolution): issued_at is an hourly FETCH-TIME proxy (±~1h, migration 005), so bin edges and the {REFERENCE_MAX_LEAD_H}h reference cutoff inherit ±1h uncertainty."
    ));
    out.log("  Δ = revision = value@lead − reference (freshest issuance per target); bins are lead-time (loH, hiH] in hours.");
    for d in &result.datasets {
        print_dataset(d, out);
    }
    out.log(&format!("\nrecommendation: {}", rec.verdict.as_str()));
    out.log(&format!("  {}", rec.reason));
    out.log("");
    if rec.verdict == Verdict::Defer {
        out.warn("DEFER — insufficient data for a verdict; exiting non-zero so this is not read as a decision.");
        return false;
    }
    true
}

/// Runs `revision-magnitude` with the arguments after the subcommand.
///
/// # Errors
/// [`ToolError`] for an unreadable fixture, a bad flag, or a database error.
pub fn run(args: &[String], ctx: &Context<'_>, out: &mut Output) -> Result<i32, ToolError> {
    if let Some(file) = flag_value(args, "--data") {
        let input = load_vintage_fixture(Path::new(file))?;
        return Ok(i32::from(!print_report(&input, out)));
    }
    if !has_flag(args, "--db") {
        out.error(USAGE);
        return Ok(2);
    }
    let window_days = parse_window_days(flag_value(args, "--window"))?;
    let url = database_url(args, ctx, "spot-price-tools revision-magnitude --db")?;
    let input = db::block_on(db::fetch_study_input(&url, window_days, ctx.now))??;
    if let Some(file) = flag_value(args, "--export") {
        std::fs::write(file, to_fixture_json(&input)).map_err(|source| ToolError::Write {
            path: file.to_owned(),
            source,
        })?;
        let count: usize = input.vintages_by_dataset.values().map(Vec::len).sum();
        out.log(&format!("Exported {count} vintages → {file}"));
    }
    Ok(i32::from(!print_report(&input, out)))
}
